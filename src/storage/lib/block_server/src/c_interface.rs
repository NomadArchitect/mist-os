// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{GroupOrRequest, IntoSessionManager, Operation, RequestId, SessionHelper};
use anyhow::Error;
use block_protocol::{BlockFifoRequest, BlockFifoResponse};
use fidl::endpoints::RequestStream;
use fuchsia_async::{self as fasync, EHandle};
use fuchsia_zircon::{self as zx, AsHandleRef as _};
use futures::stream::{AbortHandle, Abortable};
use futures::TryStreamExt;
use std::collections::{HashMap, VecDeque};
use std::ffi::{c_char, c_void, CStr};
use std::mem::MaybeUninit;
use std::sync::{Arc, Condvar, Mutex, Weak};
use {fidl_fuchsia_hardware_block as fblock, fidl_fuchsia_hardware_block_volume as fvolume};

pub struct SessionManager {
    callbacks: Callbacks,
    open_sessions: Mutex<HashMap<usize, Weak<Session>>>,
    condvar: Condvar,
    mbox: ExecutorMailbox,
}

unsafe impl Send for SessionManager {}
unsafe impl Sync for SessionManager {}

impl SessionManager {
    fn terminate(&self) {
        {
            // We must drop references to sessions whilst we're not holding the lock for
            // `open_sessions` because `Session::drop` needs to take that same lock.
            let mut terminated_sessions = Vec::new();
            for (_, session) in &*self.open_sessions.lock().unwrap() {
                if let Some(session) = session.upgrade() {
                    session.terminate();
                    terminated_sessions.push(session);
                }
            }
        }
        let _guard =
            self.condvar.wait_while(self.open_sessions.lock().unwrap(), |s| !s.is_empty()).unwrap();
    }
}

impl super::SessionManager for SessionManager {
    async fn open_session(
        self: Arc<Self>,
        mut stream: fblock::SessionRequestStream,
        block_size: u32,
    ) -> Result<(), Error> {
        let (helper, fifo) = SessionHelper::new(block_size)?;
        let (abort_handle, registration) = AbortHandle::new_pair();
        let session = Arc::new(Session {
            manager: self.clone(),
            helper,
            fifo,
            queue: Mutex::default(),
            vmos: Mutex::default(),
            abort_handle,
        });
        self.open_sessions
            .lock()
            .unwrap()
            .insert(Arc::as_ptr(&session) as usize, Arc::downgrade(&session));
        unsafe {
            (self.callbacks.on_new_session)(self.callbacks.context, Arc::into_raw(session.clone()));
        }

        let result = Abortable::new(
            async {
                while let Some(request) = stream.try_next().await? {
                    session.helper.handle_request(request)?;
                }
                Ok(())
            },
            registration,
        )
        .await
        .unwrap_or_else(|e| Err(e.into()));

        let _ = session.fifo.signal_handle(zx::Signals::empty(), zx::Signals::USER_0);

        result
    }
}

impl Drop for SessionManager {
    fn drop(&mut self) {
        self.terminate();
    }
}

impl IntoSessionManager for Arc<SessionManager> {
    type SM = SessionManager;

    fn into_session_manager(self) -> Self {
        self
    }
}

#[repr(C)]
pub struct Callbacks {
    pub context: *mut c_void,
    pub start_thread: unsafe extern "C" fn(context: *mut c_void, arg: *const c_void),
    pub on_new_session: unsafe extern "C" fn(context: *mut c_void, session: *const Session),
    pub on_requests: unsafe extern "C" fn(
        context: *mut c_void,
        session: *const Session,
        requests: *const Request,
        request_count: usize,
    ),
}

/// cbindgen:no-export
#[allow(dead_code)]
pub struct UnownedVmo(zx::sys::zx_handle_t);

#[repr(C)]
pub struct Request {
    pub request_id: RequestId,
    pub operation: Operation,
    pub vmo: UnownedVmo,
}

unsafe impl Send for Callbacks {}
unsafe impl Sync for Callbacks {}

pub struct Session {
    manager: Arc<SessionManager>,
    helper: SessionHelper,
    fifo: zx::Fifo,
    queue: Mutex<SessionQueue>,
    vmos: Mutex<HashMap<RequestId, Arc<zx::Vmo>>>,
    abort_handle: AbortHandle,
}

#[derive(Default)]
struct SessionQueue {
    responses: VecDeque<BlockFifoResponse>,
}

pub const MAX_REQUESTS: usize = 64;

impl Session {
    fn run(&self) {
        self.fifo_loop();
        self.abort_handle.abort();
    }

    fn fifo_loop(&self) {
        let mut requests = [MaybeUninit::uninit(); MAX_REQUESTS];

        loop {
            // Send queued responses.
            let is_queue_empty = {
                let mut queue = self.queue.lock().unwrap();
                while !queue.responses.is_empty() {
                    let (front, _) = queue.responses.as_slices();
                    match unsafe {
                        self.fifo.write_ptr(
                            std::mem::size_of::<BlockFifoResponse>(),
                            front.as_ptr() as *mut u8,
                            front.len(),
                        )
                    } {
                        Ok(count) => {
                            let full = count < front.len();
                            queue.responses.drain(..count);
                            if full {
                                break;
                            }
                        }
                        Err(zx::Status::SHOULD_WAIT) => break,
                        Err(_) => return,
                    }
                }
                queue.responses.is_empty()
            };

            // Process pending reads.
            match unsafe {
                self.fifo.read_ptr(
                    std::mem::size_of::<BlockFifoRequest>(),
                    requests[0].as_mut_ptr() as *mut u8,
                    MAX_REQUESTS,
                )
            } {
                Ok(count) => self.handle_requests(
                    requests[..count].iter().map(|r| unsafe { r.assume_init_ref() }),
                ),
                Err(zx::Status::SHOULD_WAIT) => {
                    let mut signals =
                        zx::Signals::OBJECT_READABLE | zx::Signals::USER_0 | zx::Signals::USER_1;
                    if !is_queue_empty {
                        signals |= zx::Signals::OBJECT_WRITABLE;
                    }
                    let Ok(signals) = self.fifo.wait_handle(signals, zx::Time::INFINITE) else {
                        return;
                    };
                    if signals.contains(zx::Signals::USER_0) {
                        return;
                    }
                    // Clear USER_1 signal if it's set.
                    if signals.contains(zx::Signals::USER_1) {
                        let _ = self.fifo.signal_handle(zx::Signals::USER_1, zx::Signals::empty());
                    }
                }
                Err(_) => return,
            }
        }
    }

    fn handle_requests<'a>(&self, requests: impl Iterator<Item = &'a BlockFifoRequest>) {
        let mut decoded_requests: [MaybeUninit<Request>; MAX_REQUESTS] =
            unsafe { MaybeUninit::uninit().assume_init() };
        let mut count = 0;
        for request in requests {
            if let Some(r) = self.helper.decode_fifo_request(request) {
                let operation = match r.operation {
                    Ok(Operation::CloseVmo) => {
                        self.send_reply(r.group_or_request, zx::Status::OK);
                        continue;
                    }
                    Ok(operation) => operation,
                    Err(status) => {
                        self.send_reply(r.group_or_request, status);
                        continue;
                    }
                };
                let mut request_id = r.group_or_request.into();
                let vmo = if let Some(vmo) = r.vmo {
                    let raw_handle = vmo.raw_handle();
                    self.vmos.lock().unwrap().insert(request_id, vmo);
                    request_id = request_id.with_vmo();
                    UnownedVmo(raw_handle)
                } else {
                    UnownedVmo(zx::sys::ZX_HANDLE_INVALID)
                };
                decoded_requests[count].write(Request { request_id, operation, vmo });
                count += 1;
            }
        }
        if count > 0 {
            unsafe {
                (self.manager.callbacks.on_requests)(
                    self.manager.callbacks.context,
                    self,
                    decoded_requests[0].as_ptr(),
                    count,
                );
            }
        }
    }

    fn send_reply(&self, id: GroupOrRequest, status: zx::Status) {
        let response = match id {
            GroupOrRequest::Group(group_id) => {
                let Some(response) = self.helper.message_groups.complete(group_id, status) else {
                    return;
                };
                response
            }
            GroupOrRequest::Request(reqid) => {
                BlockFifoResponse { status: status.into_raw(), reqid, ..Default::default() }
            }
        };
        let mut queue = self.queue.lock().unwrap();
        if queue.responses.is_empty() {
            match unsafe {
                self.fifo.write_ptr(
                    std::mem::size_of::<BlockFifoResponse>(),
                    &response as *const _ as *const u8,
                    1,
                )
            } {
                Ok(count) => {
                    assert_eq!(count, 1);
                    return;
                }
                Err(_) => {
                    // Wake `fifo_loop`.
                    let _ = self.fifo.signal_handle(zx::Signals::empty(), zx::Signals::USER_1);
                }
            }
        }
        queue.responses.push_back(response);
    }

    fn terminate(&self) {
        let _ = self.fifo.signal_handle(zx::Signals::empty(), zx::Signals::USER_0);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let notify = {
            let mut open_sessions = self.manager.open_sessions.lock().unwrap();
            open_sessions.remove(&(self as *const _ as usize));
            open_sessions.is_empty()
        };
        if notify {
            self.manager.condvar.notify_all();
        }
    }
}

pub struct BlockServer {
    server: super::BlockServer<SessionManager>,
    ehandle: EHandle,
    abort_handle: AbortHandle,
}

#[derive(Default)]
struct ExecutorMailbox(Mutex<Mail>, Condvar);

impl ExecutorMailbox {
    fn post(&self, mail: Mail) {
        *self.0.lock().unwrap() = mail;
        self.1.notify_all();
    }
}

#[derive(Default)]
enum Mail {
    #[default]
    None,
    Initialized(EHandle, AbortHandle),
    Finished,
}

impl Drop for BlockServer {
    fn drop(&mut self) {
        self.abort_handle.abort();
        let manager = &self.server.session_manager;
        let mbox = manager.mbox.0.lock().unwrap();
        let _unused = manager.mbox.1.wait_while(mbox, |mbox| !matches!(mbox, Mail::Finished));
        manager.terminate();
    }
}

#[repr(C)]
pub struct PartitionInfo {
    pub block_count: u64,
    pub block_size: u32,
    pub type_guid: [u8; 16],
    pub instance_guid: [u8; 16],
    pub name: *const c_char,
}

/// cbindgen:no-export
#[allow(non_camel_case_types)]
type zx_handle_t = zx::sys::zx_handle_t;

/// cbindgen:no-export
#[allow(non_camel_case_types)]
type zx_status_t = zx::sys::zx_status_t;

impl PartitionInfo {
    unsafe fn to_rust(&self) -> super::PartitionInfo {
        super::PartitionInfo {
            block_count: self.block_count,
            block_size: self.block_size,
            type_guid: self.type_guid,
            instance_guid: self.instance_guid,
            name: if self.name.is_null() {
                String::new()
            } else {
                String::from_utf8_lossy(CStr::from_ptr(self.name).to_bytes()).to_string()
            },
        }
    }
}

/// # Safety
///
/// All callbacks in `callbacks` must be safe.
#[no_mangle]
pub unsafe extern "C" fn block_server_new(
    partition_info: &PartitionInfo,
    callbacks: Callbacks,
) -> *mut BlockServer {
    let session_manager = Arc::new(SessionManager {
        callbacks,
        open_sessions: Mutex::default(),
        condvar: Condvar::new(),
        mbox: ExecutorMailbox::default(),
    });

    (session_manager.callbacks.start_thread)(
        session_manager.callbacks.context,
        Arc::into_raw(session_manager.clone()) as *const c_void,
    );

    let mbox = &session_manager.mbox;
    let mail = {
        let mail = mbox.0.lock().unwrap();
        std::mem::replace(
            &mut *mbox.1.wait_while(mail, |mail| matches!(mail, Mail::None)).unwrap(),
            Mail::None,
        )
    };

    match mail {
        Mail::Initialized(ehandle, abort_handle) => Box::into_raw(Box::new(BlockServer {
            server: super::BlockServer::new(partition_info.to_rust(), session_manager),
            ehandle,
            abort_handle,
        })),
        Mail::Finished => std::ptr::null_mut(),
        Mail::None => unreachable!(),
    }
}

/// # Safety
///
/// `arg` must be the value passed to the `start_thread` callback.
#[no_mangle]
pub unsafe extern "C" fn block_server_thread(arg: *const c_void) {
    let session_manager = Arc::from_raw(arg as *const SessionManager);

    let mut executor = fasync::LocalExecutor::new();
    let (abort_handle, registration) = AbortHandle::new_pair();

    session_manager.mbox.post(Mail::Initialized(EHandle::local(), abort_handle));

    let _ = executor.run_singlethreaded(Abortable::new(std::future::pending::<()>(), registration));
}

/// Called to delete the thread.  This *must* always be called, regardless of whether starting the
/// thread is successful or not.
///
/// # Safety
///
/// `arg` must be the value passed to the `start_thread` callback.
#[no_mangle]
pub unsafe extern "C" fn block_server_thread_delete(arg: *const c_void) {
    let session_manager = Arc::from_raw(arg as *const SessionManager);
    session_manager.mbox.post(Mail::Finished);
}

/// # Safety
///
/// `block_server` must be valid.
#[no_mangle]
pub unsafe extern "C" fn block_server_delete(block_server: *mut BlockServer) {
    let _ = Box::from_raw(block_server);
}

/// Serves the Volume protocol for this server.  `handle` is consumed.
///
/// # Safety
///
/// `block_server` and `handle` must be valid.
#[no_mangle]
pub unsafe extern "C" fn block_server_serve(block_server: *const BlockServer, handle: zx_handle_t) {
    let block_server = &*block_server;
    let ehandle = &block_server.ehandle;
    let handle = zx::Handle::from_raw(handle);
    ehandle
        .root_scope()
        .spawn(async move {
            let _ = block_server
                .server
                .handle_requests(fvolume::VolumeRequestStream::from_channel(
                    fasync::Channel::from_channel(handle.into()),
                ))
                .await;
        })
        .detach();
}

/// # Safety
///
/// `session` must be valid.
#[no_mangle]
pub unsafe extern "C" fn block_server_session_run(session: &Session) {
    session.run();
}

/// # Safety
///
/// `session` must be valid.
#[no_mangle]
pub unsafe extern "C" fn block_server_session_release(session: &Session) {
    session.terminate();
    Arc::from_raw(session);
}

/// # Safety
///
/// `session` must be valid.
#[no_mangle]
pub unsafe extern "C" fn block_server_send_reply(
    session: &Session,
    request_id: RequestId,
    status: zx_status_t,
) {
    if request_id.did_have_vmo() {
        session.vmos.lock().unwrap().remove(&request_id);
    }
    session.send_reply(request_id.into(), zx::Status::from_raw(status));
}

// Copyright 2024 Mist Tecnologia LTDA. All rights reserved.
// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(not(feature = "starnix_lite"))]
use anyhow::{anyhow, Context, Error};
#[cfg(feature = "starnix_lite")]
use anyhow::{anyhow, Error};
#[cfg(not(feature = "starnix_lite"))]
use bstr::BString;
#[cfg(not(feature = "starnix_lite"))]
use fuchsia_zircon as zx;
#[cfg(not(feature = "starnix_lite"))]
use gpu::gpu_device_init;
#[cfg(not(feature = "starnix_lite"))]
use gralloc::gralloc_device_init;
#[cfg(not(feature = "starnix_lite"))]
use input_device::uinput::register_uinput_device;
#[cfg(not(feature = "starnix_lite"))]
use input_device::InputDevice;
#[cfg(not(feature = "starnix_lite"))]
use input_device::InputDevice;
#[cfg(not(feature = "starnix_lite"))]
use magma_device::magma_device_init;
use selinux::security_server;
#[cfg(not(feature = "starnix_lite"))]
use starnix_core::device::android::bootloader_message_store::android_bootloader_message_store_init;
#[cfg(not(feature = "starnix_lite"))]
use starnix_core::device::ashmem::ashmem_device_init;
#[cfg(not(feature = "starnix_lite"))]
use starnix_core::device::framebuffer::{fb_device_init, AspectRatio};
#[cfg(not(feature = "starnix_lite"))]
use starnix_core::device::perfetto_consumer::start_perfetto_consumer_thread;
#[cfg(not(feature = "starnix_lite"))]
use starnix_core::device::remote_block_device::remote_block_device_init;
#[cfg(not(feature = "starnix_lite"))]
use starnix_core::device::touch_power_policy_device::TouchPowerPolicyDevice;
#[cfg(feature = "starnix_lite")]
use starnix_core::task::KernelFeatures;
#[cfg(not(feature = "starnix_lite"))]
use starnix_core::task::{CurrentTask, Kernel, KernelFeatures};
#[cfg(not(feature = "starnix_lite"))]
use starnix_core::vfs::FsString;
#[cfg(not(feature = "starnix_lite"))]
use starnix_logging::log_error;
use starnix_sync::{Locked, Unlocked};
#[cfg(not(feature = "starnix_lite"))]
use starnix_uapi::error;
#[cfg(not(feature = "starnix_lite"))]
use starnix_uapi::errors::Errno;
#[cfg(not(feature = "starnix_lite"))]
use std::sync::mpsc::channel;
#[cfg(not(feature = "starnix_lite"))]
use std::sync::Arc;

#[cfg(not(feature = "starnix_lite"))]
use {
    fidl_fuchsia_sysinfo as fsysinfo, fidl_fuchsia_ui_composition as fuicomposition,
    fidl_fuchsia_ui_input3 as fuiinput, fidl_fuchsia_ui_policy as fuipolicy,
    fidl_fuchsia_ui_views as fuiviews,
};

/// A collection of parsed features, and their arguments.
#[derive(Default, Debug)]
pub struct Features {
    pub kernel: KernelFeatures,

    /// Configures whether SELinux is fully enabled, faked, or unavailable.
    pub selinux: Option<security_server::Mode>,

    #[cfg(not(feature = "starnix_lite"))]
    pub ashmem: bool,

    #[cfg(not(feature = "starnix_lite"))]
    pub framebuffer: bool,
    #[cfg(not(feature = "starnix_lite"))]
    pub framebuffer2: bool,

    #[cfg(not(feature = "starnix_lite"))]
    pub gralloc: bool,

    #[cfg(not(feature = "starnix_lite"))]
    pub magma: bool,

    #[cfg(not(feature = "starnix_lite"))]
    pub gfxstream: bool,

    /// Include the /container directory in the root file system.
    pub container: bool,

    /// Include the /test_data directory in the root file system.
    pub test_data: bool,

    /// Include the /custom_artifacts directory in the root file system.
    pub custom_artifacts: bool,

    #[cfg(not(feature = "starnix_lite"))]
    pub android_serialno: bool,

    pub self_profile: bool,

    #[cfg(not(feature = "starnix_lite"))]
    pub aspect_ratio: Option<AspectRatio>,

    #[cfg(not(feature = "starnix_lite"))]
    pub perfetto: Option<FsString>,

    #[cfg(not(feature = "starnix_lite"))]
    pub android_fdr: bool,

    pub rootfs_rw: bool,

    pub network_manager: bool,
}

/// Parses all the featurse in `entries`.
///
/// Returns an error if parsing fails, or if an unsupported feature is present in `features`.
pub fn parse_features(entries: &Vec<String>) -> Result<Features, Error> {
    let mut features = Features::default();
    for entry in entries {
        let (raw_flag, raw_args) =
            entry.split_once(':').map(|(f, a)| (f, Some(a.to_string()))).unwrap_or((entry, None));
        match (raw_flag, raw_args) {
            #[cfg(not(feature = "starnix_lite"))]
            ("android_fdr", _) => features.android_fdr = true,
            #[cfg(not(feature = "starnix_lite"))]
            ("android_serialno", _) => features.android_serialno = true,
            #[cfg(not(feature = "starnix_lite"))]
            ("aspect_ratio", Some(args)) => {
                let e = anyhow!("Invalid aspect_ratio: {:?}", args);
                let components: Vec<_> = args.split(':').collect();
                if components.len() != 2 {
                    return Err(e);
                }
                let width: u32 = components[0].parse().map_err(|_| anyhow!("Invalid aspect ratio width"))?;
                let height: u32 = components[1].parse().map_err(|_| anyhow!("Invalid aspect ratio height"))?;
                features.aspect_ratio = Some(AspectRatio { width, height });
            }
            #[cfg(not(feature = "starnix_lite"))]
            ("aspect_ratio", None) => {
                return Err(anyhow!(
                    "Aspect ratio feature must contain the aspect ratio in the format: aspect_ratio:w:h"
                ))
            }
            ("container", _) => features.container = true,
            ("custom_artifacts", _) => features.custom_artifacts = true,
            #[cfg(not(feature = "starnix_lite"))]
            ("ashmem", _) => features.ashmem = true,
            #[cfg(not(feature = "starnix_lite"))]
            ("framebuffer", _) => features.framebuffer = true,
            #[cfg(not(feature = "starnix_lite"))]
            ("framebuffer2", _) => features.framebuffer2 = true,
            #[cfg(not(feature = "starnix_lite"))]
            ("gralloc", _) => features.gralloc = true,
            #[cfg(not(feature = "starnix_lite"))]
            ("magma", _) => features.magma = true,
            #[cfg(not(feature = "starnix_lite"))]
            ("network_manager", _) => features.network_manager = true,
            ("gfxstream", _) => features.gfxstream = true,
            #[cfg(not(feature = "starnix_lite"))]
            ("bpf", Some(version)) => features.kernel.bpf_v2 = version == "v2",
            ("enable_suid", _) => features.kernel.enable_suid = true,
            #[cfg(not(feature = "starnix_lite"))]
            ("perfetto", Some(socket_path)) => {
                features.perfetto = Some(socket_path.into());
            }
            #[cfg(not(feature = "starnix_lite"))]
            ("perfetto", None) => {
                return Err(anyhow!("Perfetto feature must contain a socket path"));
            }
            ("rootfs_rw", _) => features.rootfs_rw = true,
            ("self_profile", _) => features.self_profile = true,
            ("selinux", mode_arg) => features.selinux = match mode_arg.as_ref() {
                Some(mode) => if mode == "fake" {
                    Some(security_server::Mode::Fake)
                } else {
                    return Err(anyhow!("Invalid SELinux mode"));
                },
                None => Some(security_server::Mode::Enable),
            },
            ("test_data", _) => features.test_data = true,
            (f, _) => {
                return Err(anyhow!("Unsupported feature: {}", f));
            }
        };
    }

    Ok(features)
}

/// Runs all the features that are enabled in `system_task.kernel()`.
pub fn run_container_features(
    #[cfg(not(feature = "starnix_lite"))] locked: &mut Locked<'_, Unlocked>,
    #[cfg(not(feature = "starnix_lite"))] system_task: &CurrentTask,
    features: &Features,
) -> Result<(), Error> {
    #[cfg(not(feature = "starnix_lite"))]
    let kernel = system_task.kernel();

    let mut enabled_profiling = false;
    #[cfg(not(feature = "starnix_lite"))]
    if features.framebuffer || features.framebuffer2 {
        fb_device_init(locked, system_task);

        let (touch_source_proxy, touch_source_stream) = fidl::endpoints::create_sync_proxy();
        let view_bound_protocols = fuicomposition::ViewBoundProtocols {
            touch_source: Some(touch_source_stream),
            ..Default::default()
        };
        let view_identity = fuiviews::ViewIdentityOnCreation::from(
            fuchsia_scenic::ViewRefPair::new().expect("Failed to create ViewRefPair"),
        );
        let view_ref = fuchsia_scenic::duplicate_view_ref(&view_identity.view_ref)
            .expect("Failed to dup view ref.");
        let keyboard =
            fuchsia_component::client::connect_to_protocol_sync::<fuiinput::KeyboardMarker>()
                .expect("Failed to connect to keyboard");
        let registry_proxy = fuchsia_component::client::connect_to_protocol_sync::<
            fuipolicy::DeviceListenerRegistryMarker,
        >()
        .expect("Failed to connect to device listener registry");

        // These need to be set before `Framebuffer::start_server` is called.
        // `Framebuffer::start_server` is only called when the `framebuffer` component feature is
        // enabled. The container is the runner for said components, and `run_container_features`
        // is performed before the Container is fully initialized. Therefore, it's safe to set
        // these values at this point.
        //
        // In the future, we would like to avoid initializing a framebuffer unconditionally on the
        // Kernel, at which point this logic will need to change.
        *kernel.framebuffer.view_identity.lock() = Some(view_identity);
        *kernel.framebuffer.view_bound_protocols.lock() = Some(view_bound_protocols);

        let framebuffer = kernel.framebuffer.info.read();

        let display_width = framebuffer.xres as i32;
        let display_height = framebuffer.yres as i32;

        let touch_device =
            InputDevice::new_touch(display_width, display_height, &kernel.inspect_node);
        let keyboard_device = InputDevice::new_keyboard(&kernel.inspect_node);

        touch_device.clone().register(locked, &kernel.kthreads.system_task());
        keyboard_device.clone().register(locked, &kernel.kthreads.system_task());
        register_uinput_device(locked, &kernel.kthreads.system_task());

        touch_device.start_touch_relay(&kernel, touch_source_proxy);
        keyboard_device.start_keyboard_relay(&kernel, keyboard, view_ref);
        keyboard_device.start_button_relay(&kernel, registry_proxy);

        // Channel we use to inform the relay of changes to `touch_standby`
        let (touch_standby_sender, touch_standby_receiver) = channel::<bool>();
        let touch_policy_device = TouchPowerPolicyDevice::new(touch_standby_sender);
        touch_policy_device.clone().register(locked, &kernel.kthreads.system_task());
        touch_policy_device.start_relay(&kernel, touch_standby_receiver);

        if features.framebuffer2 {
            kernel
                .framebuffer
                .start_server(kernel, None)
                .expect("Failed to start framebuffer server");
        }
    }
    #[cfg(not(feature = "starnix_lite"))]
    if features.gralloc {
        // The virtgralloc0 device allows vulkan_selector to indicate to gralloc
        // whether swiftshader or magma will be used. This is separate from the
        // magma feature because the policy choice whether to use magma or
        // swiftshader is in vulkan_selector, and it can potentially choose
        // switfshader for testing purposes even when magma0 is present. Also,
        // it's nice to indicate swiftshader the same way regardless of whether
        // the magma feature is enabled or disabled. If a call to gralloc AIDL
        // IAllocator allocate2 occurs with this feature disabled, the call will
        // fail.
        gralloc_device_init(locked, system_task);
    }
    #[cfg(not(feature = "starnix_lite"))]
    if features.magma {
        magma_device_init(locked, system_task);
    }
    #[cfg(not(feature = "starnix_lite"))]
    if features.gfxstream {
        gpu_device_init(locked, system_task);
    }
    #[cfg(not(feature = "starnix_lite"))]
    if let Some(socket_path) = features.perfetto.clone() {
        start_perfetto_consumer_thread(kernel, socket_path)
            .context("Failed to start perfetto consumer thread")?;
    }
    if features.self_profile {
        enabled_profiling = true;
        fuchsia_inspect::component::inspector().root().record_lazy_child(
            "self_profile",
            fuchsia_inspect_contrib::ProfileDuration::lazy_node_callback,
        );
        fuchsia_inspect_contrib::start_self_profiling();
    }
    #[cfg(not(feature = "starnix_lite"))]
    if features.ashmem {
        ashmem_device_init(locked, system_task);
    }
    if !enabled_profiling {
        fuchsia_inspect_contrib::stop_self_profiling();
    }
    #[cfg(not(feature = "starnix_lite"))]
    if features.android_fdr {
        android_bootloader_message_store_init(locked, system_task);
        remote_block_device_init(locked, system_task);
    }
    if features.network_manager {
        if let Err(e) = kernel.network_manager.init() {
            log_error!("Network manager initialization failed: ({e:?})");
        }
    }

    Ok(())
}

/// Runs features requested by individual components inside the container.
#[cfg(not(feature = "starnix_lite"))]
pub fn run_component_features(
    kernel: &Arc<Kernel>,
    entries: &Vec<String>,
    mut incoming_dir: Option<fidl_fuchsia_io::DirectoryProxy>,
) -> Result<(), Errno> {
    for entry in entries {
        match entry.as_str() {
            "framebuffer" => {
                kernel
                    .framebuffer
                    .start_server(kernel, incoming_dir.take())
                    .expect("Failed to start framebuffer server");
            }
            feature => {
                return error!(ENOSYS, format!("Unsupported feature: {}", feature));
            }
        }
    }
    Ok(())
}

#[cfg(not(feature = "starnix_lite"))]
pub async fn get_serial_number() -> anyhow::Result<BString> {
    let sysinfo = fuchsia_component::client::connect_to_protocol::<fsysinfo::SysInfoMarker>()?;
    let serial = sysinfo.get_serial_number().await?.map_err(zx::Status::from_raw)?;
    Ok(BString::from(serial))
}

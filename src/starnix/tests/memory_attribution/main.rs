// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use attribution_testing::PrincipalIdentifier;
use diagnostics_reader::{ArchiveReader, Logs};
use fidl::endpoints::DiscoverableProtocolMarker;
use fidl::AsHandleRef;
use fuchsia_component_test::{
    RealmBuilder, RealmBuilderParams, RealmInstance, ScopedInstanceFactory,
};
use futures::stream::BoxStream;
use futures::StreamExt;
use moniker::Moniker;
use regex::Regex;
use zx::{MapsMappingInfo, ProcessMapsInfo};
use {
    fidl_fuchsia_component as fcomponent, fidl_fuchsia_component_decl as fdecl,
    fidl_fuchsia_io as fio, fidl_fuchsia_memory_attribution as fattribution,
    fidl_fuchsia_starnix_container as fcontainer, fidl_fuchsia_sys2 as fsys2, fuchsia_zircon as zx,
};

const PROGRAM_COLLECTION: &str = "debian_programs";

#[fuchsia::test]
async fn mmap_anonymous() {
    const PROGRAM_URL: &str = "mmap_anonymous_then_sleep_package#meta/mmap_anonymous_then_sleep.cm";
    const EXPECTED_LOG: &str = "mmap_anonymous_then_sleep did mmap";

    let AttributionTest { realm, realm_proxy, container_execution, mut attribution } =
        init_attribution_test().await;

    // Stream memory attribution data until the container shows up in the reporting.
    let mut tree: attribution_testing::Principal;
    loop {
        tree = attribution.next().await.unwrap();
        if tree.children.len() == 1 {
            break;
        }
    }
    // Starnix runner should report a single container, backed by a job.
    assert_eq!(tree.children.len(), 1);
    assert_eq!(tree.children[0].name, "debian_container");
    assert_eq!(tree.children[0].children.len(), 0);
    assert_eq!(tree.children[0].resources.len(), 1);

    // Use the container to start the Starnix program.
    let factory = ScopedInstanceFactory::new(PROGRAM_COLLECTION).with_realm_proxy(realm_proxy);
    let program = factory.new_instance(PROGRAM_URL).await.unwrap();

    // Wait for magic log from the program.
    wait_for_log(&realm, EXPECTED_LOG).await;

    // Wait for the desired attribution information.
    // There should be three child principals under the container:
    // - The init task (PID 1).
    // - The system task (PID 2).
    // - The task for the test program we just launched (PID > 2), and its name should
    //   match regex "(.*): mmap_anonymous_then_sleep".
    let program_name = Regex::new(r"(.*): mmap_anonymous_then_sleep").unwrap();
    loop {
        tree = attribution.next().await.unwrap();
        let container = &tree.children[0];
        if container
            .children
            .iter()
            .any(|child| program_name.is_match(child.name.as_str()) && child.resources.len() > 0)
        {
            break;
        }
    }
    let mut container = tree.children[0].clone();
    assert_eq!(container.children.len(), 3);
    let init = container
        .children
        .remove(container.children.iter().position(|c| c.name == "1: init").unwrap());
    assert_eq!(init.children.len(), 0);
    assert_eq!(init.resources.len(), 1, "init task should have one restricted VMAR");
    let system_task = container
        .children
        .remove(container.children.iter().position(|c| c.name == "2: [system task]").unwrap());
    assert_eq!(system_task.children.len(), 0);
    assert_eq!(system_task.resources.len(), 0, "system task should have zero restricted VMARs");
    let test_program = container.children.pop().unwrap();
    assert!(program_name.is_match(test_program.name.as_str()));
    assert_eq!(test_program.children.len(), 0);
    assert_eq!(test_program.resources.len(), 1, "test program should have one restricted VMAR");

    // We should receive the KOID of the test program process and the (base, len) information
    // of the restricted VMAR.
    //
    // The starnix process VMAR hierarchy looks like this:
    //
    //     /A ________00200000-0000400000000000     64.0T:sz                    'proc(restricted):5004903'
    //     /R ________00200000-0000400000000000     64.0T:sz                    'root'
    //     R  ________00200000-0000400000000000     64.0T:sz                    'useralloc'
    //      M 000000833189a000-000000833189b000 r--    4K:sz   0B:res 5004931:vmo 'blob-2a4670c5'
    //     ...
    //
    // and we are looking for a VMO allocated by the test program that is within
    // the restricted VMAR, with the expected number of populated pages.
    let resource = test_program.resources[0];
    let process = get_process_handle_by_name(&realm, "mmap_anonymous_then_sleep").await;
    assert_matches!(
        resource,
        attribution_testing::Resource::Vmar { process: process_koid, base, len }
        if process_koid == process.get_koid().unwrap() &&
            find_mapping_in_range(&process, |map, info| {
                // Within the restricted aspace.
                if map.base >= base && map.base + map.size <= base + len {
                    if info.populated_pages == 4200 {  // See mmap_anonymous_then_sleep.c for size.
                        return true;
                    }
                }
                false
            })
    );

    // If we terminate the program, the tree should eventually no longer contain the program.
    drop(program);
    loop {
        tree = attribution.next().await.unwrap();
        if tree.children.len() == 1 && tree.children[0].children.len() == 2 {
            break;
        }
    }

    // Stop the container and verify that the starnix runner reports that the
    // container is removed.
    container_execution.stop().unwrap();
    let event = container_execution.take_event_stream().next().await.unwrap().unwrap();
    assert_matches!(event, fcomponent::ExecutionControllerEvent::OnStop { .. });
    loop {
        tree = attribution.next().await.unwrap();
        if tree.children.is_empty() {
            break;
        }
    }
}

#[fuchsia::test]
async fn leader_killed() {
    const PROGRAM_URL: &str =
        "thread_group_leader_killed_package#meta/thread_group_leader_killed.cm";
    const EXPECTED_LOG: &str = "[thread_group_leader_killed] second thread did mmap";

    let AttributionTest { realm, realm_proxy, container_execution, mut attribution } =
        init_attribution_test().await;

    // Stream memory attribution data until the container shows up in the reporting.
    let mut tree: attribution_testing::Principal;
    loop {
        tree = attribution.next().await.unwrap();
        if tree.children.len() == 1 {
            break;
        }
    }
    // Starnix runner should report a single container, backed by a job.
    assert_eq!(tree.children.len(), 1);
    assert_eq!(tree.children[0].name, "debian_container");
    assert_eq!(tree.children[0].children.len(), 0);
    assert_eq!(tree.children[0].resources.len(), 1);

    // Use the container to start the Starnix program.
    let factory = ScopedInstanceFactory::new(PROGRAM_COLLECTION).with_realm_proxy(realm_proxy);
    let program = factory.new_instance(PROGRAM_URL).await.unwrap();

    // Wait for magic log from the program.
    wait_for_log(&realm, EXPECTED_LOG).await;

    // Wait for the desired attribution information.
    // There should be three child principals under the container:
    // - The init task (PID 1).
    // - The system task (PID 2).
    // - The task for the test program we just launched (PID > 2), and its name should
    //   match regex "(.*): thread_group_leader_killed".
    let program_name = Regex::new(r"(.*): thread_group_leader_killed").unwrap();
    loop {
        tree = attribution.next().await.unwrap();
        let container = &tree.children[0];
        if container
            .children
            .iter()
            .any(|child| program_name.is_match(child.name.as_str()) && child.resources.len() > 0)
        {
            break;
        }
    }
    let mut container = tree.children[0].clone();
    assert_eq!(container.children.len(), 3);
    container.children.remove(container.children.iter().position(|c| c.name == "1: init").unwrap());
    container
        .children
        .remove(container.children.iter().position(|c| c.name == "2: [system task]").unwrap());

    let test_program = container.children.pop().unwrap();
    assert!(program_name.is_match(test_program.name.as_str()));
    assert_eq!(test_program.children.len(), 0);
    assert_eq!(test_program.resources.len(), 1, "test program should have one restricted VMAR");

    // Termination memory reporting is tested in `mmap_anonymous`.
    drop(program);
    drop(container_execution);
}

struct AttributionTest {
    realm: RealmInstance,
    realm_proxy: fcomponent::RealmProxy,
    container_execution: fcomponent::ExecutionControllerProxy,
    attribution: BoxStream<'static, attribution_testing::Principal>,
}

async fn init_attribution_test() -> AttributionTest {
    let builder = RealmBuilder::with_params(
        RealmBuilderParams::new().from_relative_url("#meta/realm.cm").start(false),
    )
    .await
    .expect("created");
    let realm = builder.build().await.expect("build test realm");

    // Start the container and obtain its execution controller.
    let (container_controller, server_end) =
        fidl::endpoints::create_proxy::<fcomponent::ControllerMarker>().unwrap();
    let realm_proxy =
        realm.root.connect_to_protocol_at_exposed_dir::<fcomponent::RealmMarker>().unwrap();
    realm_proxy
        .open_controller(
            &fdecl::ChildRef { name: "debian_container".to_string(), collection: None },
            server_end,
        )
        .await
        .unwrap()
        .expect("open_controller");
    let (container_execution, server_end) =
        fidl::endpoints::create_proxy::<fcomponent::ExecutionControllerMarker>().unwrap();
    container_controller
        .start(fcomponent::StartChildArgs::default(), server_end)
        .await
        .unwrap()
        .expect("start debian container");

    // Connect to the attribution protocol of the starnix runner.
    let attribution_provider =
        realm.root.connect_to_protocol_at_exposed_dir::<fattribution::ProviderMarker>().unwrap();
    let introspector =
        realm.root.connect_to_protocol_at_exposed_dir::<fcomponent::IntrospectorMarker>().unwrap();
    let attribution = attribution_testing::attribute_memory(
        PrincipalIdentifier(1),
        "starnix_runner".to_string(),
        attribution_provider,
        introspector,
    );
    AttributionTest { realm, realm_proxy, container_execution, attribution }
}

async fn wait_for_log(realm: &RealmInstance, expected_log: &str) {
    let mut reader = ArchiveReader::new();
    let selector: Moniker = realm.root.moniker().parse().unwrap();
    let selector = selectors::sanitize_moniker_for_selectors(&selector.to_string());
    let selector = format!("{}/**:root", selector);
    reader.add_selector(selector);
    let mut logs = reader.snapshot_then_subscribe::<Logs>().unwrap();
    loop {
        let message = logs.next().await.unwrap().unwrap();
        if message.msg().unwrap().contains(expected_log) {
            break;
        }
    }
}

async fn get_process_handle_by_name(realm: &RealmInstance, name: &str) -> zx::Process {
    // Get the job of the starnix kernel.
    let realm_query =
        realm.root.connect_to_protocol_at_exposed_dir::<fsys2::RealmQueryMarker>().unwrap();
    let starnix_kernel_moniker = find_starnix_kernel_moniker(&realm_query).await;
    let (starnix_controller, server_end) =
        fidl::endpoints::create_proxy::<fcontainer::ControllerMarker>().unwrap();
    realm_query
        .open(
            &starnix_kernel_moniker,
            fsys2::OpenDirType::ExposedDir,
            fio::OpenFlags::empty(),
            fio::ModeType::empty(),
            fcontainer::ControllerMarker::PROTOCOL_NAME,
            server_end.into_channel().into(),
        )
        .await
        .unwrap()
        .unwrap();
    let starnix_kernel_job = starnix_controller.get_job_handle().await.unwrap().job.unwrap();

    // Find the process named with the given name in the job.
    let process_koids = starnix_kernel_job.processes().unwrap();
    for koid in process_koids {
        let process = starnix_kernel_job.get_child(&koid, zx::Rights::SAME_RIGHTS.bits()).unwrap();
        if &process.get_name().unwrap().to_string_lossy() == name {
            let process: zx::Process = process.into();
            return process;
        }
    }

    panic!("Did not find test starnix program");
}

async fn find_starnix_kernel_moniker(realm_query: &fsys2::RealmQueryProxy) -> String {
    // Enumerate the instances until we find the starnix kernel.
    let iterator = realm_query.get_all_instances().await.unwrap().unwrap();
    let iterator = iterator.into_proxy().unwrap();
    while let Ok(instances) = iterator.next().await {
        for instance in instances {
            if let Some(url) = instance.url {
                if url.contains("starnix_kernel") {
                    return instance.moniker.unwrap();
                }
            }
        }
    }

    panic!("Did not find starnix kernel");
}

/// Find a mapping in the process that satisfies the predicate.
fn find_mapping_in_range(
    process: &zx::Process,
    predicate: impl Fn(&ProcessMapsInfo, &MapsMappingInfo) -> bool,
) -> bool {
    let maps = process.info_maps_vec().unwrap();
    for map in maps {
        if let Some(info) = map.into_mapping_info() {
            if predicate(&map, &info) {
                return true;
            }
        }
    }
    false
}

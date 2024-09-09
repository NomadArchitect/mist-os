use anyhow::Result;
use fuchsia_async as fasync;

// [START example]
#[fasync::run_singlethreaded(test)]
async fn test_driver() -> Result<()> {
    let dev = fuchsia_fs::directory::open_in_namespace_deprecated(
        "/dev",
        fuchsia_fs::OpenFlags::empty(),
    )?;
    device_watcher::recursive_wait(&dev, "sys/test").await?;
    Ok(())
}
// [END example]

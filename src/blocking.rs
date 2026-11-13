/// Keep synchronous state work from occupying a networking worker. The closure
/// still finishes before its caller continues, including durable commits.
pub(crate) fn run<T>(work: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current().map(|handle| handle.runtime_flavor()) {
        Ok(tokio::runtime::RuntimeFlavor::MultiThread) => tokio::task::block_in_place(work),
        // Synchronous tools and current-thread unit tests have no worker to hand off.
        _ => work(),
    }
}

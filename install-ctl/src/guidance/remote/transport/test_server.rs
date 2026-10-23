use std::process::Child;

pub(crate) struct Server(
    pub(crate) Child,
    #[cfg(windows)] Option<super::super::process::ProcessOwner>,
);

impl Server {
    pub(crate) fn new(child: Child) -> Self {
        #[cfg(windows)]
        {
            let mut child = child;
            let owner = super::super::process::ProcessOwner::new(&child).unwrap_or_else(|error| {
                child.kill().expect("cancel fixture whose ownership failed");
                child.wait().expect("reap unowned fixture");
                panic!("cannot own fixture process tree: {error}");
            });
            Self(child, Some(owner))
        }
        #[cfg(not(windows))]
        {
            Self(child)
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        #[cfg(windows)]
        drop(self.1.take());
        #[cfg(not(windows))]
        if self.0.try_wait().expect("inspect fixture server").is_none() {
            self.0.kill().expect("stop fixture server");
        }
        self.0.wait().expect("reap fixture server");
    }
}

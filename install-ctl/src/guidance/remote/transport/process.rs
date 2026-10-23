use super::{Request, Response, exchange};
use std::{
    fs,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub(super) fn run(request: Request) -> Result<Response, String> {
    run_with_limits(request, Duration::from_secs(30), Duration::from_secs(120))
}

pub(super) fn run_with_limits(
    request: Request,
    connect_limit: Duration,
    interaction_limit: Duration,
) -> Result<Response, String> {
    let directory = tempfile::tempdir()
        .map_err(|_| "selective transport: cannot allocate exchange directory")?;
    fs::write(
        directory.path().join("request.json"),
        serde_json::to_vec(&request)
            .map_err(|_| "selective transport: cannot encode object request")?,
    )
    .map_err(|_| "selective transport: cannot write object request")?;
    let mut command = Command::new(
        std::env::current_exe()
            .map_err(|_| "selective transport: cannot resolve worker executable")?,
    );
    if cfg!(test) {
        command.args([
            "--exact",
            "guidance::remote::transport::tests::transport_worker",
        ]);
    } else {
        command.arg("--__guidance-object-worker");
    }
    command
        .env("INSTALL_CTL_OBJECT_WORKER", directory.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|_| "selective transport: cannot start isolated Git worker")?;
    let result = (|| {
        let _owner = ProcessOwner::new(&child)?;
        fs::write(directory.path().join("start"), [])
            .map_err(|_| "selective transport: cannot release isolated worker")?;
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        break Err("selective transport: isolated worker failed".into());
                    }
                    let bytes = fs::read(directory.path().join("response.json"))
                        .map_err(|_| "selective transport: worker returned no result")?;
                    break serde_json::from_slice::<Result<Response, String>>(&bytes)
                        .map_err(|_| "selective transport: worker returned an invalid result")?;
                }
                Ok(None) => {}
                Err(_) => break Err("selective transport: cannot observe isolated worker".into()),
            }
            if started.elapsed() > connect_limit && !directory.path().join("connected").exists() {
                break Err(format!(
                    "selective transport timeout: connection exceeded {connect_limit:?}"
                ));
            }
            if started.elapsed() > interaction_limit {
                break Err(format!(
                    "selective transport timeout: fetch interaction exceeded {interaction_limit:?}"
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
    })();
    if child
        .try_wait()
        .map_err(|_| "selective transport: cannot observe worker termination")?
        .is_none()
    {
        child
            .kill()
            .map_err(|_| "selective transport: cannot cancel worker")?;
    }
    child
        .wait()
        .map_err(|_| "selective transport: cannot reap worker")?;
    result
}

pub(super) fn worker_entry() {
    let Some(directory) = std::env::var_os("INSTALL_CTL_OBJECT_WORKER") else {
        return;
    };
    let directory = Path::new(&directory);
    let deadline = Instant::now() + Duration::from_secs(30);
    while !directory.join("start").exists() {
        if Instant::now() > deadline {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    let result = fs::read(directory.join("request.json"))
        .map_err(|_| "selective transport: cannot read worker request".to_owned())
        .and_then(|bytes| {
            serde_json::from_slice::<Request>(&bytes)
                .map_err(|_| "selective transport: invalid worker request".to_owned())
        })
        .and_then(|request| exchange::run(request, directory));
    let encoded = match serde_json::to_vec(&result) {
        Ok(encoded) => encoded,
        Err(_) => {
            eprintln!("selective transport: cannot encode worker result");
            return;
        }
    };
    if fs::write(directory.join("response.json"), encoded).is_err() {
        eprintln!("selective transport: cannot persist worker result");
    }
}

#[cfg(windows)]
pub(super) struct ProcessOwner(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl ProcessOwner {
    pub(super) fn new(child: &Child) -> Result<Self, String> {
        use std::{
            mem::{size_of, zeroed},
            os::windows::io::AsRawHandle,
        };
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        // The worker waits for a start file until assignment, so descendants cannot escape this job.
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err("selective transport: cannot create cancellation job".into());
            }
            let owner = Self(handle);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
                || AssignProcessToJobObject(handle, child.as_raw_handle().cast()) == 0
            {
                return Err("selective transport: cannot bind worker cancellation job".into());
            }
            Ok(owner)
        }
    }
}

#[cfg(windows)]
impl Drop for ProcessOwner {
    fn drop(&mut self) {
        // Closing our sole job handle terminates all owned descendants.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(unix)]
struct ProcessOwner(i32);

#[cfg(unix)]
impl ProcessOwner {
    fn new(child: &Child) -> Result<Self, String> {
        let pid = i32::try_from(child.id())
            .map_err(|_| "selective transport: worker process group ID is out of range")?;
        Ok(Self(pid))
    }
}

#[cfg(unix)]
impl Drop for ProcessOwner {
    fn drop(&mut self) {
        // The child created its own group before the start barrier was released.
        if unsafe { libc::kill(-self.0, libc::SIGKILL) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                eprintln!("selective transport: cannot cancel owned process group: {error}");
            }
        }
    }
}

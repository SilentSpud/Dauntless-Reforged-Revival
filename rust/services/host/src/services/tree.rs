//! A service's process tree: the service and everything it starts (the Director's game servers).
//! Windows: a job object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, so the tree also ends when this
//! process ends in any way, a hard kill included. Linux and macOS: a process group the service
//! leads; on Linux the service also gets SIGTERM if this process dies first.

#[cfg(unix)]
pub use imp::signal_name;
pub use imp::{ProcessTree, configure};

#[cfg(windows)]
#[allow(unsafe_code)] // Win32 job objects
mod imp {
    use std::ffi::c_void;
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::os::windows::process::CommandExt as _;
    use std::process::{Child, Command};

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation, QueryInformationJobObject,
        SetInformationJobObject, TerminateJobObject,
    };
    use windows::Win32::System::Threading::CREATE_NO_WINDOW;
    use windows::core::PCWSTR;

    /// No console window, and no share in this console: Ctrl+C reaches this process only, which
    /// then stops the services in order.
    pub fn configure(command: &mut Command) {
        command.creation_flags(CREATE_NO_WINDOW.0);
    }

    pub struct ProcessTree {
        job: OwnedHandle,
    }

    fn os_error(e: windows::core::Error) -> io::Error {
        io::Error::from_raw_os_error(e.code().0 & 0xFFFF)
    }

    impl ProcessTree {
        /// A new job holding the child. Its children are in it from their start (Windows 8 and
        /// later put a child in its parent's job), so a game server cannot outlive the Director.
        pub fn adopt(child: &Child) -> io::Result<Self> {
            // SAFETY: no security attributes and no name; the handle is owned from here on.
            let raw = unsafe { CreateJobObjectW(None, PCWSTR::null()) }.map_err(os_error)?;
            // SAFETY: a valid handle just returned and closed by nothing else.
            let job = unsafe { OwnedHandle::from_raw_handle(raw.0) };
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: the pointer and size describe `limits`, which outlives the call.
            unsafe {
                SetInformationJobObject(
                    HANDLE(job.as_raw_handle()),
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast::<c_void>(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            }
            .map_err(os_error)?;
            // SAFETY: both handles are open for the call (the child's is held by `child`).
            unsafe { AssignProcessToJobObject(HANDLE(job.as_raw_handle()), HANDLE(child.as_raw_handle())) }
                .map_err(os_error)?;
            Ok(Self { job })
        }

        /// Ends every process in the tree (as the Node kit's `taskkill /T /F`).
        pub fn terminate(&self) {
            // SAFETY: the job handle is open while `self` lives.
            let _ = unsafe { TerminateJobObject(HANDLE(self.job.as_raw_handle()), 1) };
        }

        pub fn kill(&self) {
            self.terminate();
        }

        /// Whether no process is left in the tree.
        pub fn is_empty(&self) -> bool {
            let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
            // SAFETY: the pointer and size describe `info`, which outlives the call.
            let queried = unsafe {
                QueryInformationJobObject(
                    Some(HANDLE(self.job.as_raw_handle())),
                    JobObjectBasicAccountingInformation,
                    (&raw mut info).cast::<c_void>(),
                    size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                    None,
                )
            };
            queried.is_ok() && info.ActiveProcesses == 0
        }
    }
    // Dropping the job's last handle ends what is still in it (JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE).
}

#[cfg(unix)]
#[allow(unsafe_code)] // kill(2) on the process group, prctl(2)
mod imp {
    use std::io;
    use std::os::unix::process::CommandExt as _;
    use std::process::{Child, Command};

    /// The service leads a new process group; on Linux it is also sent SIGTERM when the thread
    /// that started it (its supervisor's, which lives as long as it runs) goes away.
    pub fn configure(command: &mut Command) {
        command.process_group(0);
        #[cfg(target_os = "linux")]
        // SAFETY: prctl is async-signal-safe and touches only the new process.
        unsafe {
            command.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    pub struct ProcessTree {
        group: libc::pid_t,
    }

    impl ProcessTree {
        pub fn adopt(child: &Child) -> io::Result<Self> {
            let group = libc::pid_t::try_from(child.id()).map_err(|_| io::Error::other("process id out of range"))?;
            Ok(Self { group })
        }

        fn signal(&self, signal: libc::c_int) -> bool {
            // SAFETY: kill(2) with a negative pid signals the group; no memory is involved.
            unsafe { libc::kill(-self.group, signal) == 0 }
        }

        /// Asks every process in the group to stop (SIGTERM).
        pub fn terminate(&self) {
            self.signal(libc::SIGTERM);
        }

        pub fn kill(&self) {
            self.signal(libc::SIGKILL);
        }

        /// Whether no process is left in the group.
        pub fn is_empty(&self) -> bool {
            !self.signal(0)
        }
    }

    impl Drop for ProcessTree {
        /// What the service left behind when it ended (a game server, say) goes with it, as with
        /// the Windows job.
        fn drop(&mut self) {
            self.kill();
        }
    }

    pub fn signal_name(signal: i32) -> String {
        match signal {
            libc::SIGTERM => "SIGTERM".into(),
            libc::SIGKILL => "SIGKILL".into(),
            libc::SIGINT => "SIGINT".into(),
            libc::SIGHUP => "SIGHUP".into(),
            libc::SIGSEGV => "SIGSEGV".into(),
            libc::SIGABRT => "SIGABRT".into(),
            other => format!("signal {other}"),
        }
    }
}

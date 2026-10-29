//! Vista previa en vivo mientras no se graba: un ffmpeg ligero (4 fps, 480 px)
//! atado a la app con un Job object, así muere aunque la app se cierre de golpe.

use crate::{capture_input, ffmpeg, hidden, idle_preview_file, Result, PREVIEW_OUTPUT, PREVIEW_SCALE};
use std::fs;
use std::os::windows::io::AsRawHandle;
use std::process::{Child, Stdio};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

pub struct PreviewStream {
    monitor: u32,
    child: Child,
    job: Option<HANDLE>,
}

// El HANDLE del job solo se usa para cerrarlo al soltar el stream.
unsafe impl Send for PreviewStream {}

impl PreviewStream {
    pub fn start(monitor: u32) -> Result<Self> {
        let child = hidden(&ffmpeg()?)
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(capture_input(monitor, 4))
            .args(["-vf", PREVIEW_SCALE])
            .args(PREVIEW_OUTPUT)
            .arg(idle_preview_file())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        let job = unsafe { kill_on_close(&child) };
        Ok(Self { monitor, child, job })
    }

    pub fn monitor(&self) -> u32 {
        self.monitor
    }

    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for PreviewStream {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(job) = self.job.take() {
            unsafe {
                let _ = CloseHandle(job);
            }
        }
        let _ = fs::remove_file(idle_preview_file());
    }
}

unsafe fn kill_on_close(child: &Child) -> Option<HANDLE> {
    let job = CreateJobObjectW(None, None).ok()?;
    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let ok = SetInformationJobObject(
        job,
        JobObjectExtendedLimitInformation,
        &info as *const _ as *const _,
        std::mem::size_of_val(&info) as u32,
    )
    .is_ok()
        && AssignProcessToJobObject(job, HANDLE(child.as_raw_handle())).is_ok();
    if ok {
        Some(job)
    } else {
        let _ = CloseHandle(job);
        None
    }
}

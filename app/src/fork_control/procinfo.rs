use std::os::fd::RawFd;

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

pub(crate) fn foreground_pgid_of_fd(fd: RawFd) -> Option<u32> {
    nix::unistd::tcgetpgrp(fd)
        .ok()
        .map(|pid| pid.as_raw() as u32)
        .filter(|&pgid| pgid > 0)
}

pub(crate) struct ProcessTable(System);

impl ProcessTable {
    pub(crate) fn snapshot() -> Self {
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        Self(system)
    }

    pub(crate) fn name(&self, pid: u32) -> Option<String> {
        self.0
            .process(Pid::from_u32(pid))
            .map(|process| process.name().to_string_lossy().into_owned())
    }

    pub(crate) fn parent(&self, pid: u32) -> Option<u32> {
        self.0
            .process(Pid::from_u32(pid))?
            .parent()
            .map(|pid| pid.as_u32())
    }
}

// SPDX-License-Identifier: MPL-2.0
//! Platform-neutral diagnostic seam. The executable supplies native path checks.
use std::{io, path::Path, sync::OnceLock};

type BoundaryHook = fn(&str, &Path) -> io::Result<()>;
static HOOK: OnceLock<BoundaryHook> = OnceLock::new();

/// Install the executable's diagnostic implementation before starting workers.
/// This API and all boundary calls are absent without the qa-faults feature.
pub fn install_qa_save_boundary_hook(hook: BoundaryHook) -> Result<(), &'static str> {
    HOOK.set(hook).map_err(|_| "QA save boundary hook already installed")
}

pub(crate) fn hit(point: &str, target: &Path) -> io::Result<()> {
    dispatch(
        HOOK.get().copied(),
        std::env::var_os("BARELINE_QA_SAVE_ARM").is_some(),
        point,
        target,
    )
}

fn dispatch(hook: Option<BoundaryHook>, armed: bool, point: &str, target: &Path) -> io::Result<()> {
    match hook {
        Some(hook) => hook(point, target),
        None if !armed => Ok(()),
        None => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "QA save boundary implementation unavailable",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn armed_boundary_without_native_implementation_fails_closed() {
        let target = Path::new("owned-document.txt");
        dispatch(None, false, "StageFlushed", target).unwrap();
        assert_eq!(
            dispatch(None, true, "StageFlushed", target).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn installed_implementation_receives_the_exact_boundary_and_error_is_preserved() {
        fn inspect(point: &str, target: &Path) -> io::Result<()> {
            assert_eq!(point, "BeforeReplace");
            assert_eq!(target, Path::new("owned-document.txt"));
            Err(io::Error::new(io::ErrorKind::TimedOut, "diagnostic hold expired"))
        }
        assert_eq!(
            dispatch(Some(inspect), true, "BeforeReplace", Path::new("owned-document.txt"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
    }
}

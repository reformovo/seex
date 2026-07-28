use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: pulseon-viewer [PROJECT_PATH]";

fn project_path(args: impl IntoIterator<Item = OsString>) -> Result<Option<PathBuf>, ()> {
    let mut args = args.into_iter();
    let path = args.next().map(PathBuf::from);
    if args.next().is_some() {
        return Err(());
    }
    Ok(path)
}

fn main() -> ExitCode {
    let project_path = match project_path(std::env::args_os().skip(1)) {
        Ok(path) => path,
        Err(()) => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    run(project_path)
}

#[cfg(target_os = "macos")]
fn run(project_path: Option<PathBuf>) -> ExitCode {
    pulseon_viewer::desktop::run(project_path);
    ExitCode::SUCCESS
}

#[cfg(not(target_os = "macos"))]
fn run(_project_path: Option<PathBuf>) -> ExitCode {
    eprintln!("pulseon-viewer is unsupported on this platform");
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use super::project_path;

    #[test]
    fn cli_accepts_zero_or_one_project_path() {
        assert_eq!(project_path([]), Ok(None));
        assert_eq!(
            project_path([OsString::from("project")]),
            Ok(Some(PathBuf::from("project")))
        );
    }

    #[test]
    fn cli_rejects_more_than_one_project_path() {
        assert_eq!(
            project_path([OsString::from("one"), OsString::from("two")]),
            Err(())
        );
    }
}

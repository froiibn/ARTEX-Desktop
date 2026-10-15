#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod app;
mod app_icon;
mod forms;
mod graph;
mod model;
mod panes;
mod self_test;
mod theme;
mod tray;
mod window_chrome;
mod workspace_landing;

use artex_native_core::{OpenMode, OpenOptions};
use std::path::PathBuf;
use std::sync::Arc;

fn main() {
    if let Err(error) = run() {
        eprintln!("ARTEX: {error}");
        if std::env::args().len() == 1 {
            rfd::MessageDialog::new()
                .set_title("ARTEX 시작 오류")
                .set_description(format!("앱을 시작하지 못했습니다.\n\n{error}"))
                .set_level(rfd::MessageLevel::Error)
                .show();
        }
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut workspace = None;
    let mut create = false;
    let mut read_only = false;
    let mut smoke = false;
    let mut smoke_narrow = false;
    let mut smoke_target = model::Section::Overview;
    let mut smoke_dock = None;
    let mut smoke_dialog = false;
    let mut smoke_trash = false;
    let mut smoke_about = false;
    let mut smoke_hold_until = None;
    let mut self_test = false;
    let mut screenshot = None;
    let mut profile = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--workspace" => {
                workspace = Some(PathBuf::from(
                    args.next().ok_or("--workspace requires a directory")?,
                ))
            }
            "--create" => create = true,
            "--read-only" => read_only = true,
            "--smoke" => smoke = true,
            "--smoke-hold-until" => {
                smoke_hold_until = Some(PathBuf::from(
                    args.next()
                        .ok_or("--smoke-hold-until requires an absolute sentinel path")?,
                ));
            }
            "--smoke-narrow" => smoke_narrow = true,
            "--smoke-dialog" => match args.next().as_deref() {
                Some("delete-preview") => smoke_dialog = true,
                Some("trash") => smoke_trash = true,
                Some("about") => smoke_about = true,
                _ => return Err("--smoke-dialog requires delete-preview, trash or about".into()),
            },
            "--smoke-dock" => {
                smoke_dock = match args.next().as_deref() {
                    Some("activity") => Some(app::DrawerTab::Activity),
                    Some("management") => Some(app::DrawerTab::Management),
                    _ => return Err("--smoke-dock requires activity or management".into()),
                };
            }
            "--smoke-screen" => {
                smoke_target = match args.next().as_deref() {
                    Some("dashboard") => model::Section::Overview,
                    Some("tasks") => model::Section::Tasks,
                    Some("sessions") => model::Section::Sessions,
                    Some("chat") => model::Section::Chat,
                    Some("models") => model::Section::Models,
                    Some("settings") => model::Section::Settings,
                    Some("approvals") => model::Section::Approvals,
                    Some("files") => model::Section::Files,
                    Some("archives") => model::Section::Archives,
                    _ => return Err("--smoke-screen requires dashboard, tasks, sessions, chat, models, settings, approvals, files or archives".into()),
                };
            }
            "--self-test" => self_test = true,
            "--screenshot" => {
                screenshot = Some(PathBuf::from(
                    args.next().ok_or("--screenshot requires a file path")?,
                ))
            }
            "--profile" => {
                profile = Some(PathBuf::from(
                    args.next().ok_or("--profile requires a directory")?,
                ))
            }
            "--help" => {
                println!(
                    "ARTEX Rust desktop\n--workspace ABSOLUTE_PATH [--create | --read-only]\n--profile ABSOLUTE_PATH\n--self-test --workspace NEW_ABSOLUTE_PATH\n--smoke --workspace ABSOLUTE_PATH --screenshot PNG_PATH [--smoke-screen dashboard|tasks|sessions|chat|models|settings|approvals|files|archives] [--smoke-dock activity|management] [--smoke-dialog delete-preview|trash|about] [--smoke-narrow] [--smoke-hold-until NEW_ABSOLUTE_SENTINEL_PATH]"
                );
                return Ok(());
            }
            _ => return Err(format!("Unknown argument: {arg}").into()),
        }
    }
    validate_smoke_hold(smoke, smoke_hold_until.as_deref())?;
    if smoke_about && !smoke {
        return Err("--smoke-dialog about requires --smoke".into());
    }
    if smoke_dock.is_some() && !smoke {
        return Err("--smoke-dock requires --smoke".into());
    }
    if smoke_trash && (!smoke || smoke_target != model::Section::Archives) {
        return Err("--smoke-dialog trash requires --smoke-screen archives and --smoke".into());
    }
    if smoke_dialog
        && (!smoke
            || !matches!(
                smoke_target,
                model::Section::Chat | model::Section::Tasks | model::Section::Archives
            ))
    {
        return Err(
            "--smoke-dialog delete-preview requires --smoke and chat, tasks or archives".into(),
        );
    }
    if smoke_dock == Some(app::DrawerTab::Management) && smoke_target != model::Section::Chat {
        return Err("--smoke-dock management requires --smoke-screen chat".into());
    }
    if self_test {
        return self_test::run(workspace.ok_or("--self-test requires a new workspace directory")?)
            .map_err(Into::into);
    }
    if smoke_narrow && !smoke {
        return Err("--smoke-narrow requires --smoke".into());
    }
    let profile = profile
        .or_else(|| {
            directories::ProjectDirs::from("local", "ARTEX", "ARTEX Native")
                .map(|d| d.config_dir().to_path_buf())
        })
        .ok_or("OS profile directory unavailable")?;
    if create && read_only {
        return Err("--create and --read-only cannot be combined".into());
    }
    let instance = match artex_host::ProfileInstanceLock::acquire(&profile) {
        Ok(instance) => Arc::new(instance),
        Err(error) if error.code == "BUSY" => {
            artex_host::ProfileInstanceLock::handoff_with_mode(
                &profile,
                if create { None } else { workspace.as_deref() },
                read_only,
            )?;
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    let workspace = workspace.map(|path| OpenOptions {
        path,
        mode: if create {
            OpenMode::Create {
                name: "ARTEX".into(),
            }
        } else {
            OpenMode::Existing
        },
        read_only,
    });
    let options = eframe::NativeOptions {
        viewport: window_chrome::viewport(window_chrome::Platform::current(), smoke_narrow),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "ARTEX",
        options,
        Box::new(move |cc| {
            let mut app = app::NativeApp::new(cc, profile, workspace, smoke, screenshot, instance)?;
            app.smoke_target = smoke_target;
            app.smoke_dock = smoke_dock;
            app.smoke_dialog = smoke_dialog;
            app.smoke_trash = smoke_trash;
            app.smoke_about = smoke_about;
            app.smoke_hold_until = smoke_hold_until;
            Ok(Box::new(app))
        }),
    )?;
    Ok(())
}
fn validate_smoke_hold(
    smoke: bool,
    path: Option<&std::path::Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(path) = path {
        if !smoke {
            return Err("--smoke-hold-until requires --smoke".into());
        }
        if !path.is_absolute() {
            return Err("--smoke-hold-until requires an absolute sentinel path".into());
        }
        if std::fs::symlink_metadata(path).is_ok() {
            return Err("--smoke-hold-until requires a new, absent sentinel path".into());
        }
    }
    Ok(())
}
#[cfg(test)]
mod smoke_argument_tests {
    use super::*;
    #[test]
    fn hold_requires_smoke_and_a_fresh_absolute_sentinel() {
        let dir = tempfile::tempdir().unwrap();
        let sentinel = dir.path().join("release");
        assert!(validate_smoke_hold(false, Some(&sentinel)).is_err());
        assert!(validate_smoke_hold(true, Some(std::path::Path::new("relative-release"))).is_err());
        assert!(validate_smoke_hold(true, Some(&sentinel)).is_ok());
        std::fs::write(&sentinel, b"release").unwrap();
        assert!(validate_smoke_hold(true, Some(&sentinel)).is_err());
        assert!(validate_smoke_hold(false, None).is_ok());
    }
}

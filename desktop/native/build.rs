//! Embed the approved artwork and version metadata into Windows executables.
//! Uses the existing SDK/MinGW resource compiler; no build dependency is added.
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/branding/artex-app-icon.ico");
    for variable in ["RC", "WINDRES", "WindowsSdkDir", "WindowsSDKVersion"] {
        println!("cargo:rerun-if-env-changed={variable}");
    }
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let icon = root.join("assets/branding/artex-app-icon.ico");
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let numbers = ["MAJOR", "MINOR", "PATCH"]
        .map(|part| env::var(format!("CARGO_PKG_VERSION_{part}")).unwrap());
    let revision = env::var("CARGO_PKG_VERSION_PRE")
        .unwrap_or_default()
        .rsplit('.')
        .next()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(0);
    let file_version = format!("{},{},{},{revision}", numbers[0], numbers[1], numbers[2]);
    let resource = output.join("artex-icon-resource.rc");
    let source = format!(
        r#"#pragma code_page(65001)
1 ICON "{}"
1 VERSIONINFO
 FILEVERSION {file_version}
 PRODUCTVERSION {file_version}
 FILEFLAGSMASK 0x3fL
 FILEFLAGS 0x0L
 FILEOS 0x40004L
 FILETYPE 0x1L
 FILESUBTYPE 0x0L
BEGIN
 BLOCK "StringFileInfo"
 BEGIN
  BLOCK "040904b0"
  BEGIN
   VALUE "CompanyName", "ARTEX Desktop contributors\0"
   VALUE "FileDescription", "ARTEX Desktop\0"
   VALUE "FileVersion", "{version}\0"
   VALUE "InternalName", "ARTEX\0"
   VALUE "OriginalFilename", "ARTEX.exe\0"
   VALUE "ProductName", "ARTEX Desktop\0"
   VALUE "ProductVersion", "{version}\0"
   VALUE "LegalCopyright", "Autumn-27 and ARTEX Desktop contributors\0"
  END
 END
 BLOCK "VarFileInfo"
 BEGIN
  VALUE "Translation", 0x0409, 1200
 END
END
"#,
        rc_path(&icon)
    );
    fs::write(&resource, source).expect("Cannot write Windows icon resource");
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap();
    let (compiler, linked, mut command) = if target_env == "gnu" {
        let compiler = env::var_os("WINDRES")
            .or_else(|| env::var_os("RC"))
            .unwrap_or_else(|| "windres".into());
        let linked = output.join("artex-icon-resource.o");
        let architecture = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
        let format = match architecture.as_str() {
            "x86_64" => "pe-x86-64",
            "x86" => "pe-i386",
            _ => panic!("Unsupported GNU Windows resource architecture: {architecture}"),
        };
        let compiler = resolve_compiler(&compiler);
        let mut command = Command::new(&compiler);
        command
            .arg("--input")
            .arg(&resource)
            .arg("--output")
            .arg(&linked)
            .arg("--output-format=coff")
            .arg("--target")
            .arg(format);
        (compiler.into_os_string(), linked, command)
    } else if target_env == "msvc" {
        let compiler = env::var_os("RC")
            .map(PathBuf::from)
            .or_else(find_sdk_rc)
            .unwrap_or_else(|| PathBuf::from("rc.exe"));
        let linked = output.join("artex-icon-resource.res");
        let compiler = resolve_compiler(compiler.as_os_str());
        let mut command = Command::new(&compiler);
        command
            .arg("/nologo")
            .arg("/fo")
            .arg(&linked)
            .arg(&resource);
        (compiler.into_os_string(), linked, command)
    } else {
        panic!("Unsupported Windows resource toolchain: {target_env}");
    };
    let status = command.status().unwrap_or_else(|error| {
        panic!("Cannot run Windows resource compiler {compiler:?}: {error}. Install the matching MinGW tools or Windows SDK, or set WINDRES/RC.")
    });
    assert!(
        status.success() && linked.is_file(),
        "Windows icon resource compilation failed"
    );
    let record = format!(
        "{{\"target\":{},\"target_env\":{},\"compiler_path\":{},\"resource_source\":{},\"linked_resource\":{},\"icon\":{}}}\n",
        json_string(&env::var("TARGET").unwrap()),
        json_string(&target_env),
        json_string(&PathBuf::from(&compiler).to_string_lossy()),
        json_string(&resource.to_string_lossy()),
        json_string(&linked.to_string_lossy()),
        json_string(&icon.to_string_lossy()),
    );
    fs::write(output.join("artex-icon-resource-build.json"), record)
        .expect("Cannot record Windows resource compiler provenance");
    println!("cargo:rustc-link-arg-bin=artex-native={}", linked.display());
}

fn resolve_compiler(program: &std::ffi::OsStr) -> PathBuf {
    let direct = PathBuf::from(program);
    if direct.is_file() {
        return fs::canonicalize(direct).expect("Cannot resolve resource compiler");
    }
    if let Some(paths) = env::var_os("PATH") {
        for directory in env::split_paths(&paths) {
            let path = directory.join(program);
            for candidate in [path.clone(), path.with_extension("exe")] {
                if candidate.is_file() {
                    return fs::canonicalize(candidate).expect("Cannot resolve resource compiler");
                }
            }
        }
    }
    panic!("Windows resource compiler {program:?} was not found");
}

fn json_string(value: &str) -> String {
    let mut result = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            character if character < ' ' => {
                result.push_str(&format!("\\u{:04x}", character as u32))
            }
            character => result.push(character),
        }
    }
    result.push('"');
    result
}

fn rc_path(path: &Path) -> String {
    let path = path
        .to_str()
        .expect("Windows resource path must be valid Unicode");
    assert!(
        !path.contains(['\n', '\r', '"']),
        "Unsupported resource path characters"
    );
    path.replace('\\', "/")
}

fn find_sdk_rc() -> Option<PathBuf> {
    let root = env::var_os("WindowsSdkDir")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("ProgramFiles(x86)").map(|root| PathBuf::from(root).join("Windows Kits/10"))
        })?;
    if let Some(version) = env::var_os("WindowsSDKVersion") {
        let path = root.join("bin").join(version).join("x64/rc.exe");
        if path.is_file() {
            return Some(path);
        }
    }
    let mut versions: Vec<_> = fs::read_dir(root.join("bin"))
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    versions.sort();
    versions.reverse();
    versions
        .into_iter()
        .map(|path| path.join("x64/rc.exe"))
        .find(|path| path.is_file())
}

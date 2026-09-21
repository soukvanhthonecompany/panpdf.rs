use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    emit(
        "PanPDF -- read and edit PDF documents",
        "panpdf.exe",
        "PanPDF.Desktop",
    );
}

pub fn emit(description: &str, filename: &str, identity: &str) {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon = crate_dir.join("../../packaging/panpdf.ico");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    println!("cargo:rerun-if-changed={}", icon.display());

    let version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let mut parts: Vec<u32> = version
        .split(['.', '-', '+'])
        .map_while(|piece| piece.parse().ok())
        .collect();
    parts.resize(4, 0);
    let commas = parts
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");

    let manifest = out.join("panpdf.manifest");
    fs::write(&manifest, manifest_text(identity, description)).expect("writing the manifest");

    let icon_line = if icon.exists() {
        format!("1 ICON \"{}\"\n", escape(&icon))
    } else {
        println!("cargo:warning=packaging/panpdf.ico is missing; the executable will have no icon");
        String::new()
    };

    let script = out.join("panpdf.rc");
    fs::write(
        &script,
        format!(
            "{icon_line}\
             1 24 \"{manifest}\"\n\
             \n\
             1 VERSIONINFO\n\
             FILEVERSION {commas}\n\
             PRODUCTVERSION {commas}\n\
             FILEOS 0x4\n\
             FILETYPE 0x1\n\
             BEGIN\n\
             \x20 BLOCK \"StringFileInfo\"\n\
             \x20 BEGIN\n\
             \x20   BLOCK \"040904b0\"\n\
             \x20   BEGIN\n\
             \x20     VALUE \"CompanyName\", \"Soukvanhthone Company\"\n\
             \x20     VALUE \"FileDescription\", \"{description}\"\n\
             \x20     VALUE \"FileVersion\", \"{version}\"\n\
             \x20     VALUE \"InternalName\", \"{stem}\"\n\
             \x20     VALUE \"LegalCopyright\", \"AGPL-3.0-only\"\n\
             \x20     VALUE \"OriginalFilename\", \"{filename}\"\n\
             \x20     VALUE \"ProductName\", \"PanPDF\"\n\
             \x20     VALUE \"ProductVersion\", \"{version}\"\n\
             \x20   END\n\
             \x20 END\n\
             \x20 BLOCK \"VarFileInfo\"\n\
             \x20 BEGIN\n\
             \x20   VALUE \"Translation\", 0x409, 1200\n\
             \x20 END\n\
             END\n",
            manifest = escape(&manifest),
            stem = filename.trim_end_matches(".exe"),
        ),
    )
    .expect("writing the resource script");

    let msvc = env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    let object = out.join(if msvc { "panpdf.res" } else { "panpdf.o" });
    let Some(windres) = windres() else {
        println!(
            "cargo:warning=windres is missing; the executable will carry no icon, version or manifest"
        );
        return;
    };
    let run = Command::new(&windres)
        .args(["-O", if msvc { "res" } else { "coff" }])
        .arg(&script)
        .arg(&object)
        .status();
    match run {
        Ok(status) if status.success() => {
            println!("cargo:rustc-link-arg-bins={}", object.display());
        }
        Ok(status) => {
            println!("cargo:warning={windres} failed ({status}); no resources are linked in");
        }
        Err(error) => {
            println!("cargo:warning={windres} would not run ({error}); no resources are linked in");
        }
    }
}

fn windres() -> Option<String> {
    let candidates = [
        env::var("WINDRES").unwrap_or_default(),
        "x86_64-w64-mingw32-windres".into(),
        "windres".into(),
    ];
    candidates.into_iter().find(|name| {
        !name.is_empty()
            && Command::new(name)
                .arg("--version")
                .output()
                .is_ok_and(|out| out.status.success())
    })
}

fn escape(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

fn manifest_text(identity: &str, description: &str) -> String {
    MANIFEST
        .replace("{identity}", identity)
        .replace("{description}", description)
}

const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="{identity}" version="1.0.0.0" processorArchitecture="amd64"/>
  <description>{description}</description>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
      <activeCodePage xmlns="http://schemas.microsoft.com/SMI/2019/WindowsSettings">UTF-8</activeCodePage>
    </windowsSettings>
  </application>
</assembly>
"#;

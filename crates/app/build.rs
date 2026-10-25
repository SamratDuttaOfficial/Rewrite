//! On Windows, give the program its icon: an .ico made from the app's PNG icons, compiled into a resource
//! with windres (part of the MinGW toolchain).

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target != "windows" {
        return;
    }
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("..").join("..");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let mut pngs = Vec::new();
    for size in [16, 24, 32, 48, 64, 256] {
        let p = root.join(format!("assets/icons/hicolor/{size}x{size}/apps/rewrite.png"));
        println!("cargo:rerun-if-changed={}", p.display());
        match std::fs::read(&p) {
            Ok(data) => pngs.push((size, data)),
            Err(_) => return,
        }
    }
    // an .ico holding the PNGs as they are
    let mut ico = vec![0u8, 0, 1, 0];
    ico.extend_from_slice(&(pngs.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * pngs.len();
    for (size, data) in &pngs {
        let s = if *size >= 256 { 0 } else { *size as u8 };
        ico.extend_from_slice(&[s, s, 0, 0, 1, 0, 32, 0]);
        ico.extend_from_slice(&(data.len() as u32).to_le_bytes());
        ico.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += data.len();
    }
    for (_, data) in &pngs {
        ico.extend_from_slice(data);
    }
    let ico_path = out.join("rewrite.ico");
    std::fs::write(&ico_path, ico).unwrap();
    let rc = out.join("rewrite.rc");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let nums: Vec<&str> = version.split('.').collect();
    let comma = format!("{},{},{},0", nums.first().unwrap_or(&"0"), nums.get(1).unwrap_or(&"0"), nums.get(2).unwrap_or(&"0"));
    std::fs::write(
        &rc,
        format!(
            "1 ICON \"{}\"\n1 VERSIONINFO\nFILEVERSION {comma}\nPRODUCTVERSION {comma}\nBEGIN\n BLOCK \"StringFileInfo\"\n BEGIN\n  BLOCK \"040904B0\"\n  BEGIN\n   VALUE \"CompanyName\", \"Samrat Dutta\"\n   VALUE \"FileDescription\", \"Rewrite\"\n   VALUE \"FileVersion\", \"{version}\"\n   VALUE \"ProductName\", \"Rewrite\"\n   VALUE \"ProductVersion\", \"{version}\"\n   VALUE \"OriginalFilename\", \"Rewrite.exe\"\n  END\n END\n BLOCK \"VarFileInfo\"\n BEGIN\n  VALUE \"Translation\", 0x409, 1200\n END\nEND\n",
            ico_path.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    let obj = out.join("rewrite-res.o");
    let windres = ["windres", "x86_64-w64-mingw32-windres"]
        .into_iter()
        .find(|w| std::process::Command::new(w).arg("--version").output().is_ok_and(|o| o.status.success()));
    let Some(windres) = windres else {
        println!("cargo:warning=windres not found: the program is built without its icon");
        return;
    };
    let ok = std::process::Command::new(windres)
        .args(["--input", &rc.display().to_string(), "--output", &obj.display().to_string(), "--output-format=coff"])
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        println!("cargo:rustc-link-arg-bins={}", obj.display());
    } else {
        println!("cargo:warning=windres failed: the program is built without its icon");
    }
}

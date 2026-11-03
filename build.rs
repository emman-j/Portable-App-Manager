//! Icon pipeline.  Source of truth:  src/icon/icon.png
//!
//!  1. normalises it to a square 256x256 PNG in OUT_DIR  -> embedded in the program, used for the window / taskbar icon
//!  2. (Windows) builds a multi-size .ico and embeds it in the .exe -> icon shown by Explorer / shortcuts
//!
//! Problems are build ERRORS on purpose (so a missing icon is never silent).
//! Set the env var PAM_SKIP_ICON=1 to only warn instead (e.g. if the Windows SDK / rc.exe is not installed).

use std::{
    env, fs,
    io::Cursor,
    path::{Path, PathBuf},
};

const ICON: &str = "src/icon/icon.png";

fn skip_ok() -> bool {
    env::var_os("PAM_SKIP_ICON").is_some()
}

fn fail(msg: String) -> ! {
    panic!("\n\n  ICON ERROR: {msg}\n  (set PAM_SKIP_ICON=1 to build without the icon)\n");
}

fn main() {
    println!("cargo:rerun-if-changed={ICON}");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=PAM_SKIP_ICON");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let png_out = out_dir.join("icon256.png");

    let sq = match load_square_256() {
        Ok(img) => img,
        Err(e) => {
            if skip_ok() {
                println!("cargo:warning=app icon skipped: {e}");
                // still produce a valid (transparent) file so include_bytes! compiles
                let blank = image::RgbaImage::new(256, 256);
                blank.save(&png_out).unwrap();
                return;
            }
            fail(e)
        }
    };
    if let Err(e) = sq.save(&png_out) {
        fail(format!("cannot write {}: {e}", png_out.display()));
    }
    println!("cargo:warning=app icon loaded from {ICON}");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let ico = out_dir.join("app.ico");
        if let Err(e) = make_ico(&sq, &ico) {
            if skip_ok() {
                println!("cargo:warning=exe icon skipped: {e}");
            } else {
                fail(e);
            }
        } else {
            embed(&ico);
        }
    }
}

/// Read icon.png (any common image format), pad to a square, scale to 256x256.
fn load_square_256() -> Result<image::RgbaImage, String> {
    let img = image::open(ICON).map_err(|e| format!("cannot read {ICON}: {e}"))?.to_rgba8();
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Err(format!("{ICON} is empty"));
    }
    let side = w.max(h);
    let mut canvas = image::RgbaImage::new(side, side);
    image::imageops::overlay(&mut canvas, &img, ((side - w) / 2) as i64, ((side - h) / 2) as i64);
    Ok(image::imageops::resize(&canvas, 256, 256, image::imageops::FilterType::Lanczos3))
}

/// Write an .ico containing PNG-compressed frames at several sizes.
fn make_ico(sq: &image::RgbaImage, out: &Path) -> Result<(), String> {
    let mut frames: Vec<(u32, Vec<u8>)> = Vec::new();
    for size in [16u32, 24, 32, 48, 64, 128, 256] {
        let r = image::imageops::resize(sq, size, size, image::imageops::FilterType::Lanczos3);
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgba8(r)
            .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        frames.push((size, buf));
    }

    let mut data: Vec<u8> = vec![0, 0, 1, 0];
    data.extend((frames.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * frames.len() as u32;
    for (size, png) in &frames {
        let dim = if *size >= 256 { 0u8 } else { *size as u8 };
        data.extend([dim, dim, 0, 0]);
        data.extend(1u16.to_le_bytes()); // planes
        data.extend(32u16.to_le_bytes()); // bits per pixel
        data.extend((png.len() as u32).to_le_bytes());
        data.extend(offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in &frames {
        data.extend(png);
    }
    fs::write(out, data).map_err(|e| e.to_string())
}

#[cfg(windows)]
fn embed(ico: &Path) {
    let mut res = winresource::WindowsResource::new();
    res.set_icon(&ico.to_string_lossy());
    res.set("ProductName", "Portable App Manager");
    res.set("FileDescription", "Portable App Manager");
    match res.compile() {
        Ok(()) => println!("cargo:warning=exe icon embedded"),
        Err(e) => {
            let msg = format!("could not embed the icon into the exe (is the Windows SDK / rc.exe installed?): {e}");
            if skip_ok() {
                println!("cargo:warning={msg}");
            } else {
                fail(msg);
            }
        }
    }
}

#[cfg(not(windows))]
fn embed(_: &Path) {}

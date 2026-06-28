use std::hash::{Hash, Hasher};
use std::path::PathBuf;

// Generate a multi-resolution .ico from assets/icon.png and embed it as the
// application icon resource (id 1), used for the exe, tray, and window class.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon.png");

    let png = PathBuf::from("assets/icon.png");
    if !png.exists() {
        println!("cargo:warning=assets/icon.png not found; building without an icon");
        return;
    }

    let png_bytes = match std::fs::read(&png) {
        Ok(b) => b,
        Err(e) => {
            println!("cargo:warning=failed to read icon.png: {e}");
            return;
        }
    };

    // Hash the source PNG so VERSIONINFO changes when the artwork changes,
    // which helps Windows Explorer invalidate its per-exe icon cache.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    png_bytes.hash(&mut hasher);
    let icon_hash = (hasher.finish() & 0xffff) as u16;

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let ico_path = PathBuf::from(&out_dir).join("icon.ico");

    let img = match image::load_from_memory(&png_bytes) {
        Ok(i) => i.to_rgba8(),
        Err(e) => {
            println!("cargo:warning=failed to decode icon.png: {e}");
            return;
        }
    };

    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    for size in [256u32, 128, 64, 48, 32, 16] {
        let resized =
            image::imageops::resize(&img, size, size, image::imageops::FilterType::Lanczos3);
        let frame = ico::IconImage::from_rgba_data(size, size, resized.into_raw());
        match ico::IconDirEntry::encode(&frame) {
            Ok(entry) => dir.add_entry(entry),
            Err(e) => println!("cargo:warning=failed to encode {size}px icon: {e}"),
        }
    }

    match std::fs::File::create(&ico_path).and_then(|f| dir.write(f)) {
        Ok(_) => {}
        Err(e) => {
            println!("cargo:warning=failed to write icon.ico: {e}");
            return;
        }
    }

    let version = ((0u64) << 48) | ((1u64) << 32) | ((0u64) << 16) | (icon_hash as u64);
    let version_text = format!("0.1.0.{}", icon_hash);

    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico_path.to_str().unwrap());
    res.set_version_info(winresource::VersionInfo::FILEVERSION, version);
    res.set_version_info(winresource::VersionInfo::PRODUCTVERSION, version);
    res.set("FileVersion", &version_text);
    res.set("ProductVersion", &version_text);
    res.set_manifest(
        r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="requireAdministrator" uiAccess="false" />
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>"#,
    );
    // Force the resource compiler to rebuild when the icon hash changes.
    res.append_rc_content(&format!("// icon-hash: {icon_hash}\n"));
    if let Err(e) = res.compile() {
        println!("cargo:warning=failed to embed icon resource: {e}");
    }
}

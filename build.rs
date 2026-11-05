// Embed the generated Lyssna icon in the Windows executable.

include!("src/ui/icon_pixels.rs");

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rerun-if-changed=src/macos_audio.m");
        cc::Build::new()
            .file("src/macos_audio.m")
            .flag("-fobjc-arc")
            .compile("lyssna_macos_audio");
        for framework in [
            "Foundation",
            "AVFoundation",
            "CoreMedia",
            "CoreAudio",
            "AudioToolbox",
        ] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    }
    println!("cargo:rerun-if-env-changed=LYSSNA_SPICY_LYRICS_KEY");
    println!("cargo:rerun-if-env-changed=SPOTIFLEX_SPICY_LYRICS_KEY");
    println!("cargo:rerun-if-changed=config/spicy-lyrics-client-key.txt");
    // This is a public client identifier, never a secret key or Spotify token.
    // SPOTIFLEX_SPICY_LYRICS_KEY is the name from before the rename.
    let key = std::env::var("LYSSNA_SPICY_LYRICS_KEY")
        .or_else(|_| std::env::var("SPOTIFLEX_SPICY_LYRICS_KEY"))
        .ok()
        .filter(|key| !key.trim().is_empty())
        .unwrap_or_else(|| include_str!("config/spicy-lyrics-client-key.txt").into());
    let key = key.trim();
    assert!(
        key.starts_with("sl_pk_")
            && (16..=512).contains(&key.len())
            && key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte)),
        "LYSSNA_SPICY_LYRICS_KEY must contain a publishable sl_pk_ client key"
    );
    println!("cargo:rustc-env=LYSSNA_SPICY_LYRICS_KEY={key}");
    if std::env::var_os("CARGO_FEATURE_DESKTOP").is_some() {
        println!("cargo:rerun-if-changed=dist");
        tauri_build::build();
        return;
    }
    println!("cargo:rerun-if-changed=src/ui/icon_pixels.rs");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let path = out.join("lyssna.ico");
    std::fs::write(&path, ico(&[16, 24, 32, 48, 64, 128, 256])).expect("write icon");
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon(path.to_str().expect("icon path"));
    resource.set("ProductName", "Lyssna");
    resource.set("FileDescription", "Lyssna");
    if let Err(error) = resource.compile() {
        // A missing resource compiler should not block development builds.
        println!("cargo:warning=Executable icon not embedded: {error}");
    }
}

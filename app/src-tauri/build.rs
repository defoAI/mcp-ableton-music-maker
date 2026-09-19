fn main() {
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rerun-if-changed=src/listen/tap.m");
        // The Core Audio process tap. Compiled against the current SDK but
        // with a 12.0 deployment target, so everything newer is weak-imported
        // and guarded by @available: the binary still launches on macOS 12
        // and 13, where the Listen screen says what it needs.
        cc::Build::new()
            .file("src/listen/tap.m")
            .flag("-fobjc-arc")
            .flag("-fmodules")
            .flag("-mmacosx-version-min=12.0")
            .compile("amm_listen_tap");
        for framework in ["CoreAudio", "AudioToolbox", "Foundation"] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
        // The audio-capture prompt takes its text from Info.plist. Tauri
        // merges the one beside this file into the bundle's, and embeds the
        // result in the executable for development builds, so the prompt
        // appears under `cargo tauri dev` and in the tests as well.
        println!("cargo:rerun-if-changed=Info.plist");
    }
    tauri_build::build()
}

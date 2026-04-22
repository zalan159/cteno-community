fn main() {
    const FALLBACK_HAPPY_SERVER_URL: &str = "https://cteno.frontfidelity.cn";

    println!("cargo:rerun-if-env-changed=HAPPY_SERVER_URL");
    println!("cargo:rerun-if-env-changed=EXPO_PUBLIC_HAPPY_SERVER_URL");

    let compiled_default = std::env::var("HAPPY_SERVER_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("EXPO_PUBLIC_HAPPY_SERVER_URL")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| FALLBACK_HAPPY_SERVER_URL.to_string());

    println!(
        "cargo:rustc-env=CTENO_DEFAULT_HAPPY_SERVER_URL={}",
        compiled_default
    );

    // Expose the build-target triple so runtime sibling-lookup can find
    // Tauri-bundled sidecars (suffixed `{name}-{triple}`).
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown-triple".to_string());
    println!("cargo:rustc-env=TARGET_TRIPLE_FALLBACK={}", target);

    tauri_build::build()
}

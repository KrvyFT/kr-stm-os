fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rerun-if-changed=memory.x");
    // cortex-m-rt's link.x includes memory.x from the kernel crate.
    println!("cargo:rustc-link-search={manifest_dir}");
    // Only the kernel uses the cortex-m-rt startup and memory layout.
    println!("cargo:rustc-link-arg-bin=kr-stm-os=-Tlink.x");
}

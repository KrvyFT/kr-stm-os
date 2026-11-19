fn main() {
    println!("cargo:rerun-if-changed=memory.x");
    // Only the kernel uses the cortex-m-rt startup and memory layout.
    println!("cargo:rustc-link-arg-bin=kr-stm-os=-Tlink.x");
}

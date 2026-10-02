use browser_oxide::stealth::presets::chrome_153_macos;
fn main() {
    let p = chrome_153_macos();
    println!("renderer: {}", p.webgl_renderer);
    println!("deviceMemory: {}", p.device_memory);
    println!(
        "screen: {}x{} avail {}x{}+{}",
        p.screen_width,
        p.screen_height,
        p.screen_avail_width,
        p.screen_avail_height,
        p.screen_avail_top
    );
    println!("rtt: {}", p.connection_rtt);
    println!(
        "raw sysctl cpu: {:?}",
        std::process::Command::new("sysctl")
            .args(["-n", "machdep.cpu.brand_string"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
    );
}

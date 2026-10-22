fn main() {
    // The UI uses only custom widgets, so the std-widgets style is irrelevant;
    // pin one so builds are reproducible across platforms.
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint UI failed to compile");
}

mod product_cli {
    pub(super) fn run_product() {
        main();
    }

    include!("main.rs");
}

fn main() {
    if ores_clis_core::self_update::self_update_requested() {
        ores_clis_core::self_update::run_self_update_cli(
            ores_clis_core::self_update::SelfUpdateConfig::new(
                "lunatic-lorry",
                "ll-desktop-cli",
                "ll-desktop-cli",
                env!("CARGO_PKG_VERSION"),
            ),
        );
    }

    if std::env::args()
        .nth(1)
        .is_some_and(|argument| matches!(argument.as_str(), "--version" | "-V"))
    {
        println!("ll-desktop-cli {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    product_cli::run_product();
}

fn main() {
    {
        use vergen::{vergen, Config, ShaKind};
        let mut config = Config::default();
        *config.git_mut().sha_mut() = true;
        *config.git_mut().sha_kind_mut() = ShaKind::Short;
        vergen(config).expect("Unable to generate the cargo keys! Do you have git installed?");
    }
}

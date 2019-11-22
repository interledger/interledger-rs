use config::{Config, FileFormat};
use interledger::service_util::ExchangeRateProvider;
use tracing_subscriber;
use tracing::debug;

#[test]
fn deserialize_enum () {
    install_tracing_subscriber();

    let mut config = Config::new();
    let file_config_source = config::File::from_str(r#"crypto_compare: test"#, FileFormat::Yaml);
    config.merge(file_config_source).unwrap();
    let deserialized = config.try_into::<ExchangeRateProvider>();
    if let Err(config_error) = &deserialized {
        debug!(target: "interledger-cli-test", "config error: {:?}", config_error);
    }
    assert!(deserialized.is_ok());
}

pub fn install_tracing_subscriber() {
    tracing_subscriber::fmt::Subscriber::builder()
        .with_timer(tracing_subscriber::fmt::time::ChronoUtc::rfc3339())
        .with_env_filter(tracing_subscriber::filter::EnvFilter::from_default_env())
        .try_init()
        .unwrap_or(());
}

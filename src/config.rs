use anyhow::{anyhow, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;

#[derive(Serialize, Deserialize, PartialEq, Debug, Copy, Clone, Eq)]
pub enum LogLevel {
    Error,
    Warning,
    Info,
    Debug,
}

impl LogLevel {
    // convert from our log level enum to log::LevelFilter enum.
    pub fn to_level_filter(self) -> log::LevelFilter {
        match self {
            LogLevel::Error => log::LevelFilter::Error,
            LogLevel::Warning => log::LevelFilter::Warn,
            LogLevel::Info => log::LevelFilter::Info,
            LogLevel::Debug => log::LevelFilter::Debug,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_three() -> u64 {
    3u64
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct ExchangeSetting {
    pub pair: String,
    #[serde(default = "default_true")]
    pub ws_api: bool,
    #[serde(default = "default_three")]
    pub wait_secs: u64,
}

// This is the real configuration structure.
#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub struct InnerConfig {
    // trading pair: btcusdt
    // exchange: binance, bitstamp, independentreserve
    pub exchange_pair_map: HashMap<String, Vec<ExchangeSetting>>,
    // client only. server address to connect to.
    pub server_addr: Option<String>,
    // server only. address on server to bind.
    pub bind_addr: Option<String>,
    // both the client and the server will refer to this server port setting.
    pub server_port: u16,
    // output log path. None => the log won't be output to a file.
    pub log_path: Option<String>,
    // output log level. ex: Error, Warning, Info, Debug
    pub log_level: LogLevel,
}

impl Default for InnerConfig {
    fn default() -> Self {
        Self {
            exchange_pair_map: HashMap::new(),
            server_addr: Some("127.0.0.1".to_string()),
            bind_addr: Some("0.0.0.0".to_string()),
            server_port: 50051,
            log_path: Some("./test.log".to_string()),
            log_level: LogLevel::Info,
        }
    }
}

// outer config structure. Used to define the parameter input / env input of the whole program.
#[derive(Serialize, Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Config {
    #[arg(short, long, default_value_t=String::from("./config/config.yaml"))]
    pub config_path: String,
    #[arg(skip)]
    pub inner: InnerConfig,
}

impl Config {
    // load real config from the path given by parameter input / env input.
    pub fn load(&mut self) -> Result<()> {
        let f = File::open(&self.config_path).map_err(|e| anyhow!("{:?}", e))?;
        self.inner = serde_yaml::from_reader(f).map_err(|e| anyhow!("{:?}", e))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_load() {
        let mut config = Config {
            config_path: "src/test_resource/config.yaml".to_string(),
            inner: InnerConfig::default(),
        };
        let result = config.load();
        println!("{:?}", result);
        assert!(result.is_ok());
        assert_eq!(
            config.inner,
            InnerConfig {
                exchange_pair_map: HashMap::from([
                    (
                        "binance".to_string(),
                        vec![ExchangeSetting {
                            pair: "btcusdt".to_string(),
                            ws_api: false,
                            wait_secs: 3,
                        }]
                    ),
                    (
                        "bitstamp".to_string(),
                        vec![ExchangeSetting {
                            pair: "btcusd".to_string(),
                            ws_api: true,
                            wait_secs: 3,
                        }]
                    ),
                ]),
                server_addr: Some("127.0.0.1".to_string()),
                bind_addr: None,
                server_port: 50051,
                log_path: Some("test.log".to_string()),
                log_level: LogLevel::Debug,
            }
        )
    }
}

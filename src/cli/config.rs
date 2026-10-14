use std::{collections::HashSet, net::SocketAddr, path::PathBuf};

use anyhow::{Context, Ok, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Algorithm {
    RoundRobin,
    WeightedRoundRobin,
    LeastConnections,
    PowerOfTwoChoices,
    Random,
    ConsistentHash,
}

pub fn default_true() -> bool {
    true
}

pub fn default_admin_listen_address() -> SocketAddr {
    "127.0.0.1:8001".parse().expect("default admin addr parses")
}

pub fn default_max_connections() -> usize {
    1024
}

pub fn default_listen_backlog() -> u32 {
    512
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Server {
    pub address: SocketAddr,
    pub weight: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub draining: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    pub listen_address: SocketAddr,
    pub algorithm: Algorithm,
    #[serde(
        default = "default_admin_listen_address",
        alias = "management_listen_address"
    )]
    pub admin_listen_address: SocketAddr,
    #[serde(default = "default_max_connections")]
    pub max_connections: usize,
    #[serde(default = "default_listen_backlog")]
    pub listen_backlog: u32,
    pub servers: Vec<Server>,
}

impl Config {
    pub async fn load(config_path: PathBuf) -> Result<Config> {
        let config_string = tokio::fs::read_to_string(&config_path)
            .await
            .with_context(|| format!("Failed to read {}", config_path.display()))?;

        let config: Self =
            toml::from_str(&config_string).context("Config file contains invalid TOML")?;

        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.servers.is_empty() {
            anyhow::bail!("At least one server should be configured.")
        };
        if self.listen_address.port() == 0 {
            anyhow::bail!("listen_port must be greater than zero");
        }
        if self.admin_listen_address.port() == 0 {
            anyhow::bail!("admin listen port must be greater than zero");
        }
        if self.admin_listen_address == self.listen_address {
            anyhow::bail!("admin_listen_address must differ from listen_address");
        }
        if self.max_connections == 0 {
            anyhow::bail!("max_connections must be greater than zero");
        }
        if self.listen_backlog == 0 {
            anyhow::bail!("listen_backlog must be greater than zero");
        }
        let mut servers_set = HashSet::<SocketAddr>::new();
        for server in &self.servers {
            if server.weight == 0 {
                anyhow::bail!("server {} weight must be greater than zero", server.address);
            }
            if server.address.port() == 0 {
                anyhow::bail!(
                    "server : {} port should be greater than 0",
                    server.address.ip()
                )
            }

            if server.address == self.listen_address {
                anyhow::bail!(
                    "server address {} must differ from listen_address {} (self-dial loop)",
                    server.address,
                    self.listen_address
                );
            }
            if server.address == self.admin_listen_address {
                anyhow::bail!(
                    "server address {} must differ from admin_listen_address {} (raw TCP would hit the HTTP admin server)",
                    server.address,
                    self.admin_listen_address
                );
            }
            if !servers_set.insert(server.address) {
                anyhow::bail!("duplicate server address: {}", server.address);
            }
        }

        Ok(())
    }
}

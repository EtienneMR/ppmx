use std::{collections::BTreeMap, fs, io::ErrorKind, path::PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::shared::{InstalledPackage, Scope};

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct State {
    pub packages: BTreeMap<String, InstalledPackage>,
}

pub struct Database {
    path: PathBuf,
}

impl Database {
    pub fn new(path: PathBuf) -> Self {
        Database { path }
    }

    pub fn from_scope(scope: &Scope) -> anyhow::Result<Self> {
        Ok(Self::new(scope.share_path()?.join("ppmx")))
    }

    fn state_file(&self) -> PathBuf {
        self.path.join("state.json")
    }

    pub fn load_state(&self) -> Result<State> {
        if !self.state_file().exists() {
            return Ok(State::default());
        }
        let content = match fs::read_to_string(self.state_file()) {
            Ok(s) => s,
            Err(e) if e.kind() == ErrorKind::NotFound => "{}".to_string(),
            Err(e) => return Err(e).context("reading state"),
        };
        serde_json::from_str(&content).context("parsing state.json")
    }

    pub fn save_state(&self, state: &State) -> Result<()> {
        fs::create_dir_all(&self.path).context("creating database directory")?;
        let content = serde_json::to_string_pretty(state)?;
        fs::write(self.state_file(), content).context("writing state.json")
    }

    pub fn servers_dir(&self) -> PathBuf {
        self.path.join("servers")
    }

    pub fn list_servers(&self) -> Result<Vec<String>> {
        fs::create_dir_all(self.servers_dir()).ok();

        let mut servers = Vec::new();
        for entry in fs::read_dir(self.servers_dir()).context("reading servers directory")? {
            let entry = entry?;
            let name = entry.file_name();
            servers.push(name.to_string_lossy().to_string());
        }
        servers.sort();
        Ok(servers)
    }

    pub fn get_server_url(&self, name: &str) -> Result<String> {
        fs::read_to_string(self.servers_dir().join(name))
            .context(format!("reading server URL for {}", name))
    }

    pub fn add_server(&self, name: &str, url: &str) -> Result<()> {
        fs::create_dir_all(self.servers_dir()).context("creating servers directory")?;
        fs::write(self.servers_dir().join(name), url).context(format!("writing server {}", name))
    }

    pub fn remove_server(&self, name: &str) -> Result<()> {
        fs::remove_file(self.servers_dir().join(name)).context(format!("removing server {}", name))
    }

    pub fn packages_dir(&self) -> PathBuf {
        self.path.join("packages")
    }

    pub fn dump(&self) -> Result<()> {
        println!("Database at: {}", self.path.display());

        println!("\nServers:");
        for server in self.list_servers()? {
            if let Ok(url) = self.get_server_url(&server) {
                println!("  {}: {}", server, url);
            }
        }

        let state = self.load_state()?;
        println!("\nPackages:");
        for (name, pkg) in &state.packages {
            println!("  {}: {}", name, pkg.version);
            for export in &pkg.exports {
                println!("    -> {}", export.read_link()?.display());
            }
        }

        Ok(())
    }
}

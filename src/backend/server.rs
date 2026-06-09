use std::{
    ffi::{OsStr, OsString},
    fs,
    io::{BufRead, BufReader},
    path::PathBuf,
};

use anyhow::{Context, Result, bail};
use log::{debug, info, warn};

use crate::backend::Scope;

pub struct ServerList {
    scope: Scope,
}

impl ServerList {
    pub fn new(scope: Scope) -> Self {
        Self { scope }
    }

    pub fn list(&self) -> Result<Vec<OsString>> {
        let servers_dir = self.servers_path()?;
        debug!("listing servers in {:?}", &servers_dir);

        let context = || format!("listing servers in {:?}", &servers_dir);

        fs::read_dir(&servers_dir)
            .with_context(context)?
            .map(|entry| -> Result<OsString> { Ok(entry.with_context(context)?.file_name()) })
            .collect()
    }

    pub fn get_url(&self, name: &OsStr) -> Result<(String, String)> {
        debug!("reading server url of {name:?}");

        let path = self.servers_path()?.join(name);

        let context = || format!("reading server url at {path:?}");

        let file = fs::OpenOptions::new()
            .read(true)
            .open(&path)
            .with_context(context)?;
        let mut reader = BufReader::new(file);

        let mut prefix = String::new();
        reader.read_line(&mut prefix).with_context(context)?;
        prefix.truncate(prefix.trim_end().len());

        let mut suffix = String::new();
        reader.read_line(&mut suffix).with_context(context)?;
        suffix.truncate(suffix.trim_end().len());

        debug!("server {name:?} is {prefix}{{}}{suffix}");

        Ok((prefix, suffix))
    }

    pub fn add(&self, name: &OsStr, url: (&str, &str)) -> Result<()> {
        info!("updating server url of {}", name.to_string_lossy());

        let servers_path = self.servers_path()?;
        let path = servers_path.join(name);

        fs::create_dir_all(servers_path)
            .with_context(|| format!("creating parent directories of {path:?}"))?;

        fs::write(&path, format!("{}\n{}\n", url.0, url.1))
            .with_context(|| format!("updating server url at {path:?}"))
    }

    pub fn remove(&self, name: &OsStr) -> Result<()> {
        info!("removing server {}", name.to_string_lossy());

        let path = self.servers_path()?.join(name);
        fs::remove_file(&path).with_context(|| format!("removing server at {path:?}"))
    }

    pub fn find_url(
        &self,
        package_name: &str,
        http_client: &reqwest::blocking::Client,
    ) -> Result<String> {
        info!("finding server of {package_name}");
        let servers = self.list().context("listing configured servers")?;

        if servers.is_empty() {
            bail!("no servers configured");
        }

        let mut last_error: Option<String> = None;

        for server in &servers {
            let (mut url, suffix) = self
                .get_url(server)
                .with_context(|| format!("building URL for server {server:?}"))?;

            url.push_str(package_name);
            url.push_str(&suffix);

            debug!("querying server for package {package_name} at {url}");

            let response = http_client
                .head(&url)
                .send()
                .with_context(|| format!("sending HEAD request to {url}"))?;

            let status = response.status();

            if status.is_success() {
                return Ok(url);
            }

            match status {
                reqwest::StatusCode::NOT_FOUND => {
                    debug!("server at {url} returned 404 Not Found");
                }
                _ => {
                    last_error = Some(format!("{url} returned {status}"));
                    warn!("{}", last_error.as_ref().unwrap());
                }
            }
        }

        if let Some(err) = last_error {
            bail!(
                "package {package_name:?} was not found on any configured server; last error: {err}"
            );
        }

        bail!("package {package_name:?} was not found on any configured server")
    }

    fn servers_path(&self) -> Result<PathBuf> {
        Ok(self.scope.app_data_dir()?.join("servers"))
    }
}

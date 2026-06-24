use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs,
    path::PathBuf,
};

use anyhow::{Context, Result, bail};
use log::{debug, trace, warn};
use serde::{Deserialize, Serialize};
use ureq::Agent;

use crate::backend::Scope;

#[derive(Serialize, Deserialize)]
pub struct Source {
    url: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
}

impl Source {
    pub fn new(url: String) -> Self {
        Self {
            url,
            headers: BTreeMap::new(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        self.url_of("<dummy package>")
            .with_context(|| format!("failed to validate Source"))?;
        Ok(())
    }

    pub fn url_of(&self, package_name: &str) -> Result<String> {
        const TEMPLATE: &str = "{package}";
        let Some((prefix, suffix)) = self.url.split_once(TEMPLATE) else {
            bail!(
                "failed to get url of {package_name}: missing {TEMPLATE} template un url {:?}",
                self.url
            );
        };

        let mut result = String::with_capacity(prefix.len() + package_name.len() + suffix.len());
        result.push_str(prefix);
        result.push_str(package_name);
        result.push_str(suffix);

        Ok(result)
    }
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.url)
    }
}

pub fn list(scope: &Scope) -> Result<Vec<OsString>> {
    let sources_dir = sources_path(scope)?;
    trace!("listing sources in {:?}", &sources_dir);

    let context = || format!("failed to list sources in {:?}", &sources_dir);

    let mut list = fs::read_dir(&sources_dir)
        .with_context(context)?
        .map(|entry| -> Result<OsString> { Ok(entry.with_context(context)?.file_name()) })
        .collect::<Result<Vec<OsString>>>()?;

    list.sort();

    Ok(list)
}

pub fn get(name: &OsStr, scope: &Scope) -> Result<Source> {
    trace!("reading source of {name:?}");

    let path = sources_path(scope)?.join(name);

    let contents =
        fs::read_to_string(&path).with_context(|| format!("failed to read source at {path:?}"))?;

    serde_json::from_str(&contents).with_context(|| format!("parsing source at {path:?}"))
}

pub fn set(name: &OsStr, source: Source, scope: &Scope) -> Result<()> {
    debug!("updating source url of {}", name.to_string_lossy());

    let sources_path = sources_path(scope)?;
    let path = sources_path.join(name);

    let contents = serde_json::to_string_pretty(&source)?;

    fs::create_dir_all(sources_path)
        .with_context(|| format!("failed to create parent directories of {path:?}"))?;

    fs::write(&path, contents).with_context(|| format!("failed to update source url at {path:?}"))
}

pub fn remove(name: &OsStr, scope: &Scope) -> Result<()> {
    debug!("removing source {}", name.to_string_lossy());

    let path = sources_path(scope)?.join(name);
    fs::remove_file(&path).with_context(|| format!("failed to remove source at {path:?}"))
}

pub fn fetch_recipe_at(
    package_name: &str,
    source: &Source,
    http_agent: &Agent,
) -> Result<Option<(String, String)>> {
    let url = source
        .url_of(package_name)
        .with_context(|| format!("failed to build URL for source {source}"))?;

    let mut request = http_agent.get(&url);
    for (header, value) in source.headers.iter() {
        request = request.header(header, value);
    }

    match request.call() {
        Ok(mut r) => {
            let body = r
                .body_mut()
                .read_to_string()
                .with_context(|| format!("failed to read recipe of {package_name} at {url}"))?;

            trace!("fetched recipe of {package_name} at {url}:\n{body}");

            Ok(Some((body, url)))
        }
        Err(ureq::Error::StatusCode(404)) => {
            trace!("fetched recipe of {package_name} at {url}: not found");

            Ok(None)
        }
        Err(e) => Err(e).context(format!(
            "failed to query source {source} for package {package_name:?} at {url}"
        )),
    }
}

pub fn get_recipe(
    package_name: &str,
    http_agent: &Agent,
    scope: &Scope,
) -> Result<(String, String, OsString)> {
    debug!("getting recipe of {package_name}");
    let sources = list(scope).context("failed to list configured sources")?;

    if sources.is_empty() {
        bail!("no sources configured");
    }

    for source_name in sources.into_iter() {
        let source = get(&source_name, scope).with_context(|| {
            format!("failed to read source of {source_name:?} at {source_name:?}")
        })?;

        match fetch_recipe_at(package_name, &source, http_agent).with_context(|| {
            format!("failled to fetch recipe of {package_name} at {source_name:?}")
        }) {
            Ok(Some((recipe, url))) => return Ok((recipe, url, source_name)),
            Ok(None) => trace!("package {package_name} was not found at {source_name:?}"),
            Err(e) => warn!("{:?}", e),
        }
    }

    bail!("package {package_name:?} was not found on any configured source")
}

fn sources_path(scope: &Scope) -> Result<PathBuf> {
    Ok(scope.app_data_dir()?.join("sources"))
}

use std::{
    ffi::{OsStr, OsString},
    fs,
    io::{BufRead, BufReader},
    path::PathBuf,
};

use anyhow::{Context, Result, bail};
use log::{debug, trace, warn};
use ureq::Agent;

use crate::backend::Scope;

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

pub fn get_url(name: &OsStr, scope: &Scope) -> Result<(String, String)> {
    trace!("reading source url of {name:?}");

    let path = sources_path(scope)?.join(name);

    let context = || format!("failed to read source url at {path:?}");

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

    trace!("source {name:?} is {prefix}{{}}{suffix}");

    Ok((prefix, suffix))
}

pub fn add(name: &OsStr, url: (&str, &str), scope: &Scope) -> Result<()> {
    debug!("updating source url of {}", name.to_string_lossy());

    let sources_path = sources_path(scope)?;
    let path = sources_path.join(name);

    fs::create_dir_all(sources_path)
        .with_context(|| format!("failed to create parent directories of {path:?}"))?;

    fs::write(&path, format!("{}\n{}\n", url.0, url.1))
        .with_context(|| format!("failed to update source url at {path:?}"))
}

pub fn remove(name: &OsStr, scope: &Scope) -> Result<()> {
    debug!("removing source {}", name.to_string_lossy());

    let path = sources_path(scope)?.join(name);
    fs::remove_file(&path).with_context(|| format!("failed to remove source at {path:?}"))
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

    for source in sources.into_iter() {
        let (mut url, suffix) = get_url(&source, scope)
            .with_context(|| format!("failed to build URL for source {source:?}"))?;

        url.push_str(package_name);
        url.push_str(&suffix);

        trace!("querying source {source:?} for package {package_name} at {url}");

        let response = http_agent.get(&url).call();

        let result: Result<Option<String>> = match response {
            Ok(mut r) => r
                .body_mut()
                .read_to_string()
                .with_context(|| format!("failed to read recipe of {package_name} at {url}"))
                .map(Some),
            Err(ureq::Error::StatusCode(404)) => Ok(None),
            Err(e) => Err(e).context(format!(
                "failed to query source {source:?} for package {package_name} at {url}"
            )),
        };

        match result {
            Ok(Some(recipe)) => return Ok((recipe, url, source)),
            Ok(None) => {
                trace!("source at {url} returned 404 Not Found");
            }
            Err(e) => {
                warn!("{:?}", e)
            }
        }
    }

    bail!("package {package_name:?} was not found on any configured source")
}

fn sources_path(scope: &Scope) -> Result<PathBuf> {
    Ok(scope.app_data_dir()?.join("sources"))
}

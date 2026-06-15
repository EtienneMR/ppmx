use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{Context, Result, bail};
use log::{debug, info, trace};
use ureq::Agent;

use crate::backend::{
    ResolvedPackage, ResolvedPackageWithDependencies, Scope,
    database::{Database, InstallRequest},
};

pub struct InstallPlan<'a> {
    plan: Vec<InstallRequest>,
    map: HashMap<String, usize>,
    visiting: HashSet<String>,

    scope: &'a Scope,
    database: &'a Database,
    http_agent: &'a Agent,
}

impl<'a> InstallPlan<'a> {
    pub fn new(scope: &'a Scope, database: &'a Database, http_agent: &'a Agent) -> Self {
        trace!("creating new installation plan");
        Self {
            plan: Vec::new(),
            map: HashMap::new(),
            visiting: HashSet::new(),
            scope,
            database,
            http_agent,
        }
    }

    pub fn add_install(&mut self, package_name: String, reinstall: bool) -> Result<()> {
        debug!("add_install request: {package_name:?} (reinstall: {reinstall})");

        if let Some(idx) = self.map.get(&package_name) {
            trace!("marking {package_name} as explicitly requested");
            self.plan[*idx].install_explicitly = true;
        } else {
            trace!("failed to plan install request for {package_name}");
            self.add(
                MaybeResolvedPackage::NameOnly(package_name),
                true,
                reinstall,
            )
            .context("failed to resolve install request")?;
        }

        Ok(())
    }

    pub fn add_update(
        &mut self,
        package_name: String,
        current_version: &str,
        recipe_url: String,
        explicit: bool,
    ) -> Result<()> {
        debug!(
            "add_update request: {:?} (current: v{}, url: {:?})",
            package_name, current_version, recipe_url
        );

        let package = ResolvedPackage::fetch(package_name, recipe_url, self.http_agent.clone())?;

        let new_version = package.version();

        if new_version != current_version {
            trace!("{} can be updated to v{}", package, new_version);
            self.add(MaybeResolvedPackage::Resolved(package), explicit, true)
                .context("failed to plan update request")?;
        } else {
            info!("{package} is already up-to-date");
        }

        Ok(())
    }

    pub fn to_plan(self) -> Vec<InstallRequest> {
        debug!(
            "exporting installation plan: {}",
            self.plan
                .iter()
                .map(|p| format!(
                    "{} ({})",
                    p.package,
                    if p.install_explicitly { "exp" } else { "dep" }
                ))
                .collect::<Vec<String>>()
                .join(" -> ")
        );

        self.plan
    }

    fn add(
        &mut self,
        package: MaybeResolvedPackage,
        explicit: bool,
        reinstall: bool,
    ) -> Result<()> {
        let package_name = package.name();

        if self.visiting.contains(package_name) {
            let chain: Vec<&str> = self.visiting.iter().map(|s| s.as_str()).collect();
            bail!(
                "circular dependency detected: {package_name:?} is already in chain: {}",
                chain.join(", ")
            );
        }

        if self.map.get(package_name).is_some() {
            trace!("{package_name:?} already in plan, skipping");
            return Ok(());
        }

        if self.database.contains_key(package_name) && !reinstall {
            debug!("{package_name:?} is already installed and reinstall=false, skipping");
            return Ok(());
        }

        let key = package_name.to_owned();
        self.visiting.insert(key.clone());
        trace!("visiting {package_name:?}");

        let resolved_package = package.resolve_with_dependencies(self.http_agent, self.scope)?;
        let package_name = resolved_package.name();

        debug!(
            "resolved {resolved_package} with {} dependencies",
            resolved_package.dependencies.len()
        );

        for dep_name in resolved_package.dependencies.iter() {
            trace!("processing dependency {dep_name:?} of {package_name:?}");

            self.add(
                MaybeResolvedPackage::NameOnly(dep_name.clone()),
                false,
                false,
            )
            .with_context(|| {
                format!("failed to process dependency {dep_name:?} (required by {package_name:?})")
            })?;
        }

        let key = self
            .visiting
            .take(package_name)
            .expect("was inserted at start of function");

        debug!("adding {resolved_package} to plan (explicit: {explicit})");

        let idx = self.plan.len();
        self.plan.push(InstallRequest {
            package: resolved_package,
            install_explicitly: explicit,
        });

        let maybe_old = self.map.insert(key.clone(), idx);
        debug_assert!(
            maybe_old.is_none(),
            "package {key:?} already added to plan at index {maybe_old:?}",
        );

        Ok(())
    }
}

pub fn plan_uninstall(database: &Database) -> Vec<&String> {
    let mut dependent_counts: HashMap<&String, usize> = HashMap::new();

    for (package_name, package) in database.iter() {
        dependent_counts.entry(package_name).or_insert(0);

        for dependency in package.dependencies() {
            dependent_counts
                .entry(dependency)
                .and_modify(|v| *v += 1)
                .or_insert(1);
        }
    }

    debug!("built dependent counts map {dependent_counts:?}");

    let mut ready = VecDeque::new();
    for (package_name, package) in database.iter() {
        if dependent_counts[package_name] == 0 && !package.explicitly_installed() {
            debug!("identified initial orphan {package_name:?}");
            ready.push_back(package_name);
        }
    }

    let mut plan = Vec::new();

    while let Some(package_name) = ready.pop_front() {
        plan.push(package_name);

        for dependency in database[package_name].dependencies() {
            let count = dependent_counts.get_mut(dependency).unwrap();
            *count -= 1;

            if *count == 0 && !database[dependency].explicitly_installed() {
                debug!("identified {dependency:?} as an unused dependency of {package_name:?}");
                ready.push_back(dependency);
            }
        }
    }

    trace!(
        "final plan {}",
        plan.iter()
            .map(|s| s.as_str())
            .collect::<Vec<&str>>()
            .join(" -> ")
    );

    plan
}

enum MaybeResolvedPackage {
    NameOnly(String),
    Resolved(ResolvedPackage),
}

impl MaybeResolvedPackage {
    fn name(&self) -> &str {
        match self {
            MaybeResolvedPackage::NameOnly(n) => n,
            MaybeResolvedPackage::Resolved(p) => p.name(),
        }
    }

    fn resolve_with_dependencies(
        self,
        http_agent: &Agent,
        scope: &Scope,
    ) -> Result<ResolvedPackageWithDependencies> {
        let package = match self {
            MaybeResolvedPackage::Resolved(p) => p,
            MaybeResolvedPackage::NameOnly(n) => {
                ResolvedPackage::resolve(n, http_agent.clone(), scope)?
            }
        };

        package.try_into()
    }
}

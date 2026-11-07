//! The destination registry (ADR 014): every Destination, Shortener and Sharer by id. `with_catalog`
//! fills it from the bundled files and the sharers, and `add_custom` adds a user's `.sxcu` file.

use std::fmt;
use std::sync::Arc;

use laqta_ports::{Destination, Prompt, Rng, Sharer, Shortener};

use crate::catalog::catalog;
use crate::sharers::sharers;
use crate::sxcu::{CustomUploader, DestinationType, SxcuError};
use crate::sxcu_destination::{SxcuDestination, SxcuShortener};

/// Hosts by id. An id is unique within each kind: adding one that is already there replaces the
/// earlier entry.
pub struct Registry {
    prompt: Arc<dyn Prompt>,
    rng: Arc<dyn Rng>,
    destinations: Vec<Arc<dyn Destination>>,
    shorteners: Vec<Arc<dyn Shortener>>,
    sharers: Vec<Arc<dyn Sharer>>,
}

impl Registry {
    /// An empty registry. `prompt` and `rng` go to every `.sxcu` host that the registry builds.
    pub fn new(prompt: Arc<dyn Prompt>, rng: Arc<dyn Rng>) -> Self {
        Registry { prompt, rng, destinations: Vec::new(), shorteners: Vec::new(), sharers: Vec::new() }
    }

    /// A registry with every bundled `.sxcu` file and every sharer.
    pub fn with_catalog(prompt: Arc<dyn Prompt>, rng: Arc<dyn Rng>) -> Self {
        let mut registry = Registry::new(prompt, rng);
        for (id, uploader) in catalog() {
            registry.add_uploader(id, uploader);
        }
        for sharer in sharers() {
            registry.add_sharer(sharer);
        }
        registry
    }

    pub fn add_destination(&mut self, destination: Arc<dyn Destination>) {
        let id = destination.id().to_owned();
        match self.destinations.iter().position(|known| known.id() == id.as_str()) {
            Some(index) => self.destinations[index] = destination,
            None => self.destinations.push(destination),
        }
    }

    pub fn add_shortener(&mut self, shortener: Arc<dyn Shortener>) {
        let id = shortener.id().to_owned();
        match self.shorteners.iter().position(|known| known.id() == id.as_str()) {
            Some(index) => self.shorteners[index] = shortener,
            None => self.shorteners.push(shortener),
        }
    }

    pub fn add_sharer(&mut self, sharer: Arc<dyn Sharer>) {
        let id = sharer.id().to_owned();
        match self.sharers.iter().position(|known| known.id() == id.as_str()) {
            Some(index) => self.sharers[index] = sharer,
            None => self.sharers.push(sharer),
        }
    }

    /// Adds a user's `.sxcu` file (ShareX format, any version). Its id is `custom:<Name>`, and a file
    /// with the same name replaces the earlier one. Returns the id.
    pub fn add_custom(&mut self, json: &str) -> Result<String, SxcuError> {
        let uploader = CustomUploader::parse(json)?;
        let name = match uploader.name.as_deref().map(str::trim) {
            Some(name) if !name.is_empty() => name.to_owned(),
            _ => return Err(SxcuError::Json("the file has no `Name`".into())),
        };
        let id = format!("custom:{name}");
        if !self.add_uploader(id.clone(), uploader) {
            return Err(SxcuError::Unsupported("the file's DestinationType names no kind this app uploads to".into()));
        }
        Ok(id)
    }

    /// Registers a `.sxcu` file as a destination (image, text or file kinds) and, for `UrlShortener`,
    /// as a shortener. Returns whether it was registered as either.
    fn add_uploader(&mut self, id: String, uploader: CustomUploader) -> bool {
        let kinds = uploader.destination_types();
        let file_host = kinds.iter().any(|kind| matches!(kind, DestinationType::Image | DestinationType::Text | DestinationType::File));
        let shortener = kinds.contains(&DestinationType::UrlShortener);
        if file_host {
            let destination = SxcuDestination::new(id.clone(), uploader.clone(), Arc::clone(&self.prompt), Arc::clone(&self.rng));
            self.add_destination(Arc::new(destination));
        }
        if shortener {
            let short = SxcuShortener::new(id, uploader, Arc::clone(&self.prompt), Arc::clone(&self.rng));
            self.add_shortener(Arc::new(short));
        }
        file_host || shortener
    }

    pub fn destination(&self, id: &str) -> Option<Arc<dyn Destination>> {
        self.destinations.iter().find(|entry| entry.id() == id).cloned()
    }

    pub fn shortener(&self, id: &str) -> Option<Arc<dyn Shortener>> {
        self.shorteners.iter().find(|entry| entry.id() == id).cloned()
    }

    pub fn sharer(&self, id: &str) -> Option<Arc<dyn Sharer>> {
        self.sharers.iter().find(|entry| entry.id() == id).cloned()
    }

    /// Every destination as (id, name), in registration order.
    pub fn destination_ids(&self) -> Vec<(String, String)> {
        self.destinations.iter().map(|entry| (entry.id().to_owned(), entry.name().to_owned())).collect()
    }
}

impl fmt::Debug for Registry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Registry")
            .field("destinations", &self.destinations.len())
            .field("shorteners", &self.shorteners.len())
            .field("sharers", &self.sharers.len())
            .finish_non_exhaustive()
    }
}

//! The destination registry (ADR 014): every Destination, Shortener and Sharer by id. `with_catalog`
//! fills it from the bundled files and the sharers, and `add_custom` adds a user's `.sxcu` file.

use std::fmt;
use std::sync::Arc;

use laqta_ports::{Destination, Prompt, Rng, Sharer, Shortener};

use crate::catalog::catalog;
use crate::sharers::sharers;
use crate::sxcu::{CustomUploader, DestinationType, SxcuError};
use crate::sxcu_destination::{SxcuDestination, SxcuSharer, SxcuShortener};

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
        // ShareX leaves `Name` out when it equals the request URL's host, and shows the host instead
        // (`CustomUploaderItem.ToString`).
        let name = match uploader.name.as_deref().map(str::trim) {
            Some(name) if !name.is_empty() => name.to_owned(),
            _ => url::Url::parse(&uploader.request_url)
                .ok()
                .and_then(|url| url.host_str().map(without_www))
                .filter(|host| !host.is_empty())
                .ok_or_else(|| SxcuError::Json("the file has no `Name` and no host in `RequestURL`".into()))?,
        };
        let id = format!("custom:{name}");
        if !self.add_uploader(id.clone(), uploader) {
            return Err(SxcuError::Unsupported("the file's DestinationType names no kind this app uploads to".into()));
        }
        Ok(id)
    }

    /// Registers a `.sxcu` file as a destination (image, text or file kinds), as a shortener for
    /// `UrlShortener` and as a sharer for `URLSharingService`. Returns whether it was registered.
    fn add_uploader(&mut self, id: String, uploader: CustomUploader) -> bool {
        let kinds = uploader.destination_types();
        let file_host = kinds.iter().any(|kind| matches!(kind, DestinationType::Image | DestinationType::Text | DestinationType::File));
        let shortener = kinds.contains(&DestinationType::UrlShortener);
        let sharer = kinds.contains(&DestinationType::UrlSharing);
        if !(file_host || shortener || sharer) {
            return false;
        }
        // A file that replaces one of the same name drops the kinds it no longer declares.
        if !file_host {
            self.destinations.retain(|entry| entry.id() != id);
        }
        if !shortener {
            self.shorteners.retain(|entry| entry.id() != id);
        }
        if !sharer {
            self.sharers.retain(|entry| entry.id() != id);
        }
        if file_host {
            let destination = SxcuDestination::new(id.clone(), uploader.clone(), Arc::clone(&self.prompt), Arc::clone(&self.rng));
            self.add_destination(Arc::new(destination));
        }
        if shortener {
            let short = SxcuShortener::new(id.clone(), uploader.clone(), Arc::clone(&self.prompt), Arc::clone(&self.rng));
            self.add_shortener(Arc::new(short));
        }
        if sharer {
            self.add_sharer(Arc::new(SxcuSharer::new(id, uploader, Arc::clone(&self.prompt), Arc::clone(&self.rng))));
        }
        true
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

    /// The user's own `.sxcu` shorteners as (id, name).
    pub fn custom_shortener_ids(&self) -> Vec<(String, String)> {
        custom_ids(self.shorteners.iter().map(|entry| entry.id()))
    }

    /// The user's own `.sxcu` sharing services as (id, name).
    pub fn custom_sharer_ids(&self) -> Vec<(String, String)> {
        custom_ids(self.sharers.iter().map(|entry| entry.id()))
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

/// `custom:<Name>` ids as (id, Name).
fn custom_ids<'a>(ids: impl Iterator<Item = &'a str>) -> Vec<(String, String)> {
    ids.filter_map(|id| id.strip_prefix("custom:").map(|name| (id.to_owned(), name.to_owned()))).collect()
}

/// A host without a leading `www.`, as ShareX's `URLHelpers.GetHostName` gives it.
fn without_www(host: &str) -> String {
    match host.get(..4) {
        Some(prefix) if prefix.eq_ignore_ascii_case("www.") => host[4..].to_owned(),
        _ => host.to_owned(),
    }
}

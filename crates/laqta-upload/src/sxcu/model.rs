//! The `.sxcu` model: typed values, the ShareX field names, and the load path.

use std::fmt;

use serde::Deserialize;
use serde::de::{MapAccess, Visitor};

use super::SxcuError;
use super::migrate;

/// A parsed `.sxcu` custom uploader. Field names follow ShareX; the request and response
/// templates are kept as written (the syntax is evaluated per upload).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomUploader {
    pub version: Option<String>,
    pub name: Option<String>,
    pub destination_types: Vec<DestinationType>,
    pub request_method: RequestMethod,
    pub request_url: String,
    pub parameters: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: BodyType,
    pub arguments: Vec<(String, String)>,
    pub file_form_name: Option<String>,
    pub data: Option<String>,
    pub url: Option<String>,
    pub thumbnail_url: Option<String>,
    pub deletion_url: Option<String>,
    pub error_message: Option<String>,
}

impl CustomUploader {
    /// Parses a `.sxcu` file. ShareX 12 files are migrated to the current syntax on the way.
    pub fn parse(json: &str) -> Result<CustomUploader, SxcuError> {
        let raw: Raw = serde_json::from_str(json).map_err(|e| SxcuError::Json(e.to_string()))?;
        raw.into_uploader()
    }

    /// The destination kinds this uploader serves, in file order.
    pub fn destination_types(&self) -> Vec<DestinationType> {
        self.destination_types.clone()
    }
}

/// The destination kinds named in `DestinationType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DestinationType {
    Image,
    Text,
    File,
    UrlShortener,
    UrlSharing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyType {
    None,
    MultipartFormData,
    FormUrlEncoded,
    Json,
    Xml,
    Binary,
}

impl RequestMethod {
    fn parse(name: &str) -> Result<Self, SxcuError> {
        match name.trim().to_ascii_uppercase().as_str() {
            "GET" => Ok(RequestMethod::Get),
            "POST" => Ok(RequestMethod::Post),
            "PUT" => Ok(RequestMethod::Put),
            "PATCH" => Ok(RequestMethod::Patch),
            "DELETE" => Ok(RequestMethod::Delete),
            _ => Err(SxcuError::Unsupported(format!("request method `{name}`"))),
        }
    }
}

impl BodyType {
    fn parse(name: &str) -> Result<Self, SxcuError> {
        match name.trim().to_ascii_lowercase().as_str() {
            "none" => Ok(BodyType::None),
            "multipartformdata" => Ok(BodyType::MultipartFormData),
            "formurlencoded" => Ok(BodyType::FormUrlEncoded),
            "json" => Ok(BodyType::Json),
            "xml" => Ok(BodyType::Xml),
            "binary" => Ok(BodyType::Binary),
            _ => Err(SxcuError::Unsupported(format!("body type `{name}`"))),
        }
    }
}

fn parse_destination_types(list: &str) -> Vec<DestinationType> {
    let mut found = Vec::new();
    for name in list.split(',') {
        let kind = match name.trim().to_ascii_lowercase().as_str() {
            "imageuploader" => DestinationType::Image,
            "textuploader" => DestinationType::Text,
            "fileuploader" => DestinationType::File,
            "urlshortener" => DestinationType::UrlShortener,
            "urlsharingservice" => DestinationType::UrlSharing,
            _ => continue,
        };
        if !found.contains(&kind) {
            found.push(kind);
        }
    }
    found
}

/// The file as written. Maps keep their file order, which serde_json's `Value` would sort.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(super) struct Raw {
    version: Option<String>,
    name: Option<String>,
    destination_type: Option<String>,
    request_method: Option<String>,
    /// ShareX 12 name for `RequestMethod`.
    request_type: Option<String>,
    #[serde(rename = "RequestURL")]
    request_url: String,
    parameters: Option<Pairs>,
    headers: Option<Pairs>,
    body: Option<String>,
    arguments: Option<Pairs>,
    file_form_name: Option<String>,
    data: Option<String>,
    #[serde(rename = "URL")]
    url: Option<String>,
    #[serde(rename = "ThumbnailURL")]
    thumbnail_url: Option<String>,
    #[serde(rename = "DeletionURL")]
    deletion_url: Option<String>,
    error_message: Option<String>,
    /// ShareX 12 response kind; `Redirection` means the final URL is the result.
    response_type: Option<String>,
    /// ShareX 12 regex list, referenced by `$regex:N,G$`.
    regex_list: Option<Vec<String>>,
}

impl Raw {
    /// Whether the file uses the ShareX 12 format, so its literal text must be escaped and its
    /// `$name$` markers converted. Modern files are never converted.
    fn is_legacy(&self) -> bool {
        let major = self.version.as_deref().and_then(|v| v.split('.').next()).and_then(|m| m.trim().parse::<u32>().ok());
        if self.request_type.is_some() || self.regex_list.as_ref().is_some_and(|list| !list.is_empty()) || major.is_some_and(|m| m < 13) {
            return true;
        }
        let mut texts: Vec<&str> = vec![self.request_url.as_str()];
        for pairs in [&self.parameters, &self.headers, &self.arguments].into_iter().flatten() {
            for (key, value) in &pairs.0 {
                texts.push(key.as_str());
                texts.push(value.as_str());
            }
        }
        texts.extend(
            [
                self.data.as_deref(),
                self.url.as_deref(),
                self.thumbnail_url.as_deref(),
                self.deletion_url.as_deref(),
                self.error_message.as_deref(),
            ]
            .into_iter()
            .flatten(),
        );
        // ShareX matches the two `Data` markers ignoring case, so `$INPUT$` marks a legacy file too.
        let data = self.data.as_deref().map(str::to_ascii_lowercase).unwrap_or_default();
        texts.into_iter().any(migrate::has_marker) || data.contains("$input$") || data.contains("$filename$")
    }

    pub(super) fn into_uploader(self) -> Result<CustomUploader, SxcuError> {
        let legacy = self.is_legacy();
        let Raw {
            version,
            name,
            destination_type,
            request_method,
            request_type,
            request_url,
            parameters,
            headers,
            body,
            arguments,
            file_form_name,
            data,
            url,
            thumbnail_url,
            deletion_url,
            error_message,
            response_type,
            regex_list,
        } = self;
        let regexes: Vec<String> = regex_list.unwrap_or_default();
        let text = Text { legacy, regexes: &regexes };

        let request_url = text.apply(request_url)?;
        let parameters = text.pairs(parameters)?;
        let headers = text.pairs(headers)?;
        let arguments = text.pairs(arguments)?;
        let data = text.data(data);
        let mut url = text.opt(url)?;
        let thumbnail_url = text.opt(thumbnail_url)?;
        let deletion_url = text.opt(deletion_url)?;
        let error_message = text.opt(error_message)?;
        if response_type.as_deref().is_some_and(|kind| kind.eq_ignore_ascii_case("redirection")) && url.as_deref().is_none_or(str::is_empty)
        {
            url = Some("{responseurl}".to_string());
        }

        let request_method = match request_method.or(request_type) {
            Some(name) => RequestMethod::parse(&name)?,
            None => RequestMethod::Post,
        };
        let body = match body {
            Some(name) => BodyType::parse(&name)?,
            None if file_form_name.is_some() => BodyType::MultipartFormData,
            None => BodyType::None,
        };
        let destination_types = destination_type.as_deref().map(parse_destination_types).unwrap_or_default();

        Ok(CustomUploader {
            version,
            name,
            destination_types,
            request_method,
            request_url,
            parameters,
            headers,
            body,
            arguments,
            file_form_name,
            data,
            url,
            thumbnail_url,
            deletion_url,
            error_message,
        })
    }
}

/// Applies the legacy conversion to literal text when the file is in the ShareX 12 format.
struct Text<'a> {
    legacy: bool,
    regexes: &'a [String],
}

impl Text<'_> {
    fn apply(&self, value: String) -> Result<String, SxcuError> {
        if self.legacy { migrate::convert(&value, self.regexes) } else { Ok(value) }
    }

    fn opt(&self, value: Option<String>) -> Result<Option<String>, SxcuError> {
        value.map(|v| self.apply(v)).transpose()
    }

    /// `Data` is ShareX text, not a template: a legacy file only gets its two marker conversions.
    fn data(&self, value: Option<String>) -> Option<String> {
        value.map(|v| if self.legacy { migrate::convert_data(&v) } else { v })
    }

    fn pairs(&self, pairs: Option<Pairs>) -> Result<Vec<(String, String)>, SxcuError> {
        let mut out = Vec::new();
        for (key, value) in pairs.map(|p| p.0).unwrap_or_default() {
            out.push((self.apply(key)?, self.apply(value)?));
        }
        Ok(out)
    }
}

/// A JSON object of strings, kept in file order.
pub(super) struct Pairs(pub(super) Vec<(String, String)>);

impl<'de> Deserialize<'de> for Pairs {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(PairsVisitor)
    }
}

struct PairsVisitor;

impl<'de> Visitor<'de> for PairsVisitor {
    type Value = Pairs;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Pairs, A::Error> {
        let mut out = Vec::new();
        while let Some((key, value)) = map.next_entry::<String, serde_json::Value>()? {
            match value {
                serde_json::Value::Null => {}
                serde_json::Value::String(text) => out.push((key, text)),
                other => out.push((key, other.to_string())),
            }
        }
        Ok(Pairs(out))
    }
}

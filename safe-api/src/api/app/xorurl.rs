// Copyright 2020 MaidSafe.net limited.
//
// This SAFE Network Software is licensed to you under the MIT license <LICENSE-MIT
// http://opensource.org/licenses/MIT> or the Modified BSD license <LICENSE-BSD
// https://opensource.org/licenses/BSD-3-Clause>, at your option. This file may not be copied,
// modified, or distributed except according to those terms. Please review the Licences for the
// specific language governing permissions and limitations relating to use of the SAFE Network
// Software.

use super::{
    nrs::NRS_MAP_TYPE_TAG,
    xorurl_media_types::{MEDIA_TYPE_CODES, MEDIA_TYPE_STR},
    DEFAULT_XORURL_BASE,
};
use crate::{Error, Result};
use log::{debug, info, warn};
use multibase::{decode, encode, Base};
use safe_nd::{XorName, XOR_NAME_LEN};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::iter::FromIterator;
use tiny_keccak::sha3_256;
use url::Url;

const SAFE_URL_PROTOCOL: &str = "safe://";
const SAFE_URL_SCHEME: &str = "safe";
const XOR_URL_VERSION_1: u64 = 0x1; // TODO: consider using 16 bits
const XOR_URL_STR_MAX_LENGTH: usize = 44;
const XOR_NAME_BYTES_OFFSET: usize = 4; // offset where to find the XoR name bytes
const URL_VERSION_QUERY_NAME: &str = "v";

// The XOR-URL type
pub type XorUrl = String;

// Backwards compatibility for the rest of codebase.
// A later PR will:
//  1. rename this file to safeurl.rs
//  2. change all references in other files
//  3. remove this alias.
pub type XorUrlEncoder = SafeUrl;

// Supported base encoding for XOR URLs
#[derive(Copy, Clone, Debug)]
pub enum XorUrlBase {
    Base32z,
    Base32,
    Base64,
}

impl std::str::FromStr for XorUrlBase {
    type Err = Error;
    fn from_str(str: &str) -> Result<Self> {
        match str {
            "base32z" => Ok(Self::Base32z),
            "base32" => Ok(Self::Base32),
            "base64" => Ok(Self::Base64),
            other => Err(Error::InvalidInput(format!(
                "Invalid XOR URL base encoding: {}. Supported values are base32z, base32, and base64",
                other
            ))),
        }
    }
}

impl fmt::Display for XorUrlBase {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl XorUrlBase {
    pub fn from_u8(value: u8) -> Result<Self> {
        match value {
            0 => Ok(Self::Base32z),
            1 => Ok(Self::Base32),
            2 => Ok(Self::Base64),
            _other => Err(Error::InvalidInput("Invalid XOR URL base encoding code. Supported values are 0=base32z, 1=base32, and 2=base64".to_string())),
        }
    }

    pub fn from_u16(value: u16) -> Result<Self> {
        match value {
            0 => Ok(Self::Base32z),
            1 => Ok(Self::Base32),
            2 => Ok(Self::Base64),
            _other => Err(Error::InvalidInput("Invalid XOR URL base encoding code. Supported values are 0=base32z, 1=base32, and 2=base64".to_string())),
        }
    }
}

// We encode the content type that a XOR-URL is targetting, this allows the consumer/user to
// treat the content in particular ways when the content requires it.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub enum SafeContentType {
    Raw,
    Wallet,
    FilesContainer,
    NrsMapContainer,
    MediaType(String),
}

impl std::fmt::Display for SafeContentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl SafeContentType {
    pub fn from_u16(value: u16) -> Result<Self> {
        match value {
            0 => Ok(Self::Raw),
            1 => Ok(Self::Wallet),
            2 => Ok(Self::FilesContainer),
            3 => Ok(Self::NrsMapContainer),
            _other => Err(Error::InvalidInput("Invalid Media-type code".to_string())),
        }
    }

    pub fn value(&self) -> Result<u16> {
        match &*self {
            Self::Raw => Ok(0),
            Self::Wallet => Ok(1),
            Self::FilesContainer => Ok(2),
            Self::NrsMapContainer => Ok(3),
            Self::MediaType(media_type) => match MEDIA_TYPE_CODES.get(media_type) {
                Some(media_type_code) => Ok(*media_type_code),
                None => Err(Error::InvalidMediaType(format!("Media-type '{}' not supported. You can use 'SafeContentType::Raw' as the 'content_type' for this type of content", media_type))),
            },
        }
    }
}

// We also encode the native SAFE data type where the content is being stored on the SAFE Network,
// this allows us to fetch the targetted data using the corresponding API, regardless of the
// data that is being held which is identified by the SafeContentType instead.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub enum SafeDataType {
    SafeKey = 0x00,
    PublishedImmutableData = 0x01,
    UnpublishedImmutableData = 0x02,
    SeqMutableData = 0x03,
    UnseqMutableData = 0x04,
    PublishedSeqAppendOnlyData = 0x05,
    PublishedUnseqAppendOnlyData = 0x06,
    UnpublishedSeqAppendOnlyData = 0x07,
    UnpublishedUnseqAppendOnlyData = 0x08,
}

impl std::fmt::Display for SafeDataType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl SafeDataType {
    pub fn from_u64(value: u64) -> Result<Self> {
        match value {
            0 => Ok(Self::SafeKey),
            1 => Ok(Self::PublishedImmutableData),
            2 => Ok(Self::UnpublishedImmutableData),
            3 => Ok(Self::SeqMutableData),
            4 => Ok(Self::UnseqMutableData),
            5 => Ok(Self::PublishedSeqAppendOnlyData),
            6 => Ok(Self::PublishedUnseqAppendOnlyData),
            7 => Ok(Self::UnpublishedSeqAppendOnlyData),
            8 => Ok(Self::UnpublishedUnseqAppendOnlyData),
            _ => Err(Error::InvalidInput("Invalid SafeDataType code".to_string())),
        }
    }
}

// A simple struct to represent the basic components parsed
// from a Safe URL without any decoding.
//
// This is kept internal to the crate, at least for now.
#[derive(Debug, Clone)]
pub(crate) struct SafeUrlParts {
    pub scheme: String,
    pub public_name: String, // "a.b.name" in "a.b.name"
    pub top_name: String,    // "name"     in "a.b.name"
    pub sub_names: String,   // "a.b"      in "a.b.name"
    pub sub_names_vec: Vec<String>,
    pub path: String,
    pub query_string: String,
    pub fragment: String,
}

impl SafeUrlParts {
    // parses a URL into its component parts, performing basic validation.
    pub fn parse(url: &str) -> Result<Self> {
        let parsing_url = Url::parse(&url).map_err(|parse_err| {
            let msg = format!("Problem parsing the URL \"{}\": {}", url, parse_err);
            Error::InvalidXorUrl(msg)
        })?;

        // Validate the url scheme is 'safe'
        let scheme = parsing_url.scheme().to_string();
        if scheme != SAFE_URL_SCHEME {
            let msg = format!(
                "invalid scheme: '{}'. expected: '{}'",
                scheme, SAFE_URL_SCHEME
            );
            return Err(Error::InvalidXorUrl(msg));
        }

        // validate name (url host) is not empty
        let public_name = match parsing_url.host_str() {
            Some(h) => h.to_string(),
            None => {
                let msg = format!("Problem parsing the URL \"{}\": {}", url, "missing name");
                return Err(Error::InvalidXorUrl(msg));
            }
        };

        // validate no empty sub names in name.
        if public_name.find("..").is_some() {
            let msg = "name contains empty subname".to_string();
            return Err(Error::InvalidXorUrl(msg));
        }

        // validate no spaces in name.
        if public_name.find(' ').is_some() {
            let msg = "name cannot contain a space".to_string();
            return Err(Error::InvalidXorUrl(msg));
        }

        // parse top_name and sub_names from name
        let names_vec = Vec::from_iter(public_name.split('.').map(String::from));
        let top_name = names_vec[names_vec.len() - 1].to_string();
        let sub_names_vec = (&names_vec[0..names_vec.len() - 1]).to_vec();
        let sub_names = sub_names_vec.join(".");

        // get path, query_params, and fragment
        let path = parsing_url.path().to_string();
        let query_string = parsing_url.query().unwrap_or("").to_string();
        let fragment = parsing_url.fragment().unwrap_or("").to_string();

        // double-slash is allowed but discouraged in regular URLs.
        // We don't allow them in Safe URLs.
        // See https://stackoverflow.com/questions/20523318/is-a-url-with-in-the-path-section-valid
        if path.find("//").is_some() {
            let msg = "path contains empty component".to_string();
            return Err(Error::InvalidXorUrl(msg));
        }

        debug!(
            "Parsed url: scheme: {}, public_name: {}, top_name: {}, sub_names: {}, sub_names_vec: {:?}, path: {}, query_string: {}, fragment: {}",
            scheme,
            public_name,
            top_name,
            sub_names,
            sub_names_vec,
            path,
            query_string,
            fragment,
        );

        let s = Self {
            scheme,
            public_name,
            sub_names,
            sub_names_vec,
            top_name,
            path,
            query_string,
            fragment,
        };

        Ok(s)
    }
}

/// An enumeration of possible SafeUrl types.
///
/// This is the type of safe url itself,
/// not the content it points to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SafeUrlType {
    XorUrl,
    NrsUrl,
}

impl SafeUrlType {
    pub fn value(&self) -> Result<u16> {
        match &*self {
            Self::XorUrl => Ok(0),
            Self::NrsUrl => Ok(1),
        }
    }
}

/// Represents a SafeUrl
///
/// A SafeUrl can be in one of two formats:  nrs or xor.
///   aka:  nrsurl or xorurl
///
/// Here is a breakdown of how name terminology is used.
///
/// case 1: safe://a.b.shinything:
///
///   public_name()    --> a.b.shinything
///   top_name()  --> shinything
///   sub_names()    --> a.b
///
/// case 2: safe://shinything:
///
///   public_name()   --> shinything
///   top_name() --> shinything
///   sub_names()   --> None
///
/// case 3: safe://a.b.hnyynyzhjjjatqkfkjux8maaojtj8r59aphcnue6a11qgecpcebidkywmybnc
///
///   public_name()   --> a.b.hnyynyzhjjjatqkfkjux8maaojtj8r59aphcnue6a11qgecpcebidkywmybnc
///   top_name() --> hnyynyzhjjjatqkfkjux8maaojtj8r59aphcnue6a11qgecpcebidkywmybnc
///   sub_names()   --> a.b
///
/// case 4: safe://hnyynyzhjjjatqkfkjux8maaojtj8r59aphcnue6a11qgecpcebidkywmybnc
///   public_name()   --> hnyynyzhjjjatqkfkjux8maaojtj8r59aphcnue6a11qgecpcebidkywmybnc
///   top_name() --> hnyynyzhjjjatqkfkjux8maaojtj8r59aphcnue6a11qgecpcebidkywmybnc
///   sub_names()   --> None
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SafeUrl {
    encoding_version: u64,      // currently only v1 supported
    xor_name: XorName,          // applies to nrsurl and xorurl
    public_name: String,        // "a.b.name" in "a.b.name"
    top_name: String,           // "name" in "a.b.name"
    sub_names: String,          // "a.b" in "a.b.name"
    sub_names_vec: Vec<String>, // vec!["a", "b"] in "a.b.name"
    type_tag: u64,
    data_type: SafeDataType,       // See SafeDataType
    content_type: SafeContentType, // See SafeContentTYpe
    content_type_u16: u16,         // validated u16 id of content_type
    path: String,                  // path, no separator, percent-encoded
    query_string: String,          // query-string, no separator, url-encoded
    fragment: String,              // fragment, no separator
    content_version: Option<u64>,  // convenience for ?v=<version
    safeurl_type: SafeUrlType,     // nrsurl or xorurl
}

/// This implementation performs semi-rigorous validation,
/// when parsing a URL using ::from_url(), ::from_xorurl(),
/// or ::from_nrsurl().
///
/// However setters and new() do not enforce all the rules
/// and using them with invalid input can result in serializing
/// invalid URLs.  GIGO.
///
/// As such, it is recommended to check validity by
/// calling SafeUrl::validate() after instantiating
/// or modifying.
///
// TBD: In the future, we may want to perform all validity
// checks in the setters, however, this requires modifying
// setters to return a Result, which potentially impacts a
// bunch of code elsewhere.
impl SafeUrl {
    #[allow(clippy::too_many_arguments)]
    /// Instantiates a new SafeUrl
    ///
    /// Performs some basic validation checks, however it is
    /// possible to create invalid urls using this method.
    ///
    /// Arguments
    /// * `xor_name` - XorName hash
    /// * `nrs_name` - complete nrs name, or None for xorurl
    /// * `type_tag` - type tag
    /// * `data_type` - SafeDataType
    /// * `content_type` - SafeContentType
    /// * `path` - must already be percent-encoded if Some. leading '/' optional.
    /// * `xorurl_sub_names` - sub_names. ignored if nrs_name is present.
    /// * `query_string` - must already be percent-encoded, without ? separator
    /// * `fragment` - url fragment, without # separator
    /// * `content_version` - overrides value of "?v" in query-string if not None.
    pub fn new(
        xor_name: XorName,
        nrs_name: Option<&str>,
        type_tag: u64,
        data_type: SafeDataType,
        content_type: SafeContentType,
        path: Option<&str>,
        sub_names: Option<Vec<String>>,
        query_string: Option<&str>,
        fragment: Option<&str>,
        content_version: Option<u64>,
    ) -> Result<Self> {
        let content_type_u16 = content_type.value()?;

        let public_name: String;
        let top_name: String;
        let sub_names_str: String;
        let sub_names_vec: Vec<String>;
        let safeurl_type: SafeUrlType;
        match nrs_name {
            Some(nh) => {
                // we have an nrsurl
                if nh.is_empty() {
                    let msg = "nrs_name cannot be empty string.".to_string();
                    return Err(Error::InvalidInput(msg));
                }
                // Validate that nrs_name hash matches xor_name
                let tmpurl = format!("{}{}", SAFE_URL_PROTOCOL, nh);
                let parts = SafeUrlParts::parse(&tmpurl)?;
                let hashed_name = Self::xor_name_from_nrs_string(&parts.top_name)?;
                if hashed_name != xor_name {
                    let msg = format!(
                        "input mis-match. nrs_name `{}` does not hash to xor_name `{}`",
                        parts.top_name, xor_name
                    );
                    return Err(Error::InvalidInput(msg));
                }
                public_name = parts.public_name;
                top_name = parts.top_name;
                sub_names_str = parts.sub_names;
                sub_names_vec = parts.sub_names_vec; // use sub_names from nrs_name, ignoring sub_names arg, in case they do not match.
                safeurl_type = SafeUrlType::NrsUrl;
            }
            None => {
                // we have an xorurl
                public_name = String::default(); // set later
                top_name = String::default(); // set later
                sub_names_vec = sub_names.unwrap_or_else(Vec::new);
                sub_names_str = sub_names_vec.join(".");
                safeurl_type = SafeUrlType::XorUrl;

                for s in &sub_names_vec {
                    if s.is_empty() {
                        let msg = "empty subname".to_string();
                        return Err(Error::InvalidInput(msg));
                    }
                }
            }
        }

        // finally, instantiate.
        let mut x = Self {
            encoding_version: XOR_URL_VERSION_1,
            xor_name,
            public_name,
            top_name,
            sub_names: sub_names_str,
            sub_names_vec,
            type_tag,
            data_type,
            content_type,
            content_type_u16,
            path: String::default(),         // set below.
            query_string: String::default(), // set below.
            fragment: fragment.unwrap_or("").to_string(),
            content_version: None, // set below.
            safeurl_type,
        };

        // now we can call ::name_to_base(), to generate the top_name.
        if x.safeurl_type == SafeUrlType::XorUrl {
            x.top_name = x.name_to_base(DEFAULT_XORURL_BASE, false);
            let sep = if x.sub_names.is_empty() { "" } else { "." };
            x.public_name = format!("{}{}{}", x.sub_names(), sep, x.top_name);
        }

        // we call this to add leading slash if needed
        // but we do NOT want percent-encoding as caller
        // must already provide it that way.
        x.set_path_internal(path.unwrap_or(""), false);

        // we set query_string and content_version using setters to
        // ensure they are in sync.
        x.set_query_string(query_string.unwrap_or(""))?;

        // If present, content_version will override ?v in query string.
        if let Some(version) = content_version {
            x.set_content_version(Some(version));
        }
        Ok(x)
    }

    // A non-member utility function to check if a media-type is currently supported by XOR-URL encoding
    pub fn is_media_type_supported(media_type: &str) -> bool {
        MEDIA_TYPE_CODES.get(media_type).is_some()
    }

    /// Parses a safe url into SafeUrl
    ///
    /// # Arguments
    ///
    /// * `url` - either nrsurl or xorurl
    pub fn from_url(url: &str) -> Result<Self> {
        match Self::from_xorurl(url) {
            Ok(enc) => Ok(enc),
            Err(err) => {
                info!(
                    "Falling back to NRS. XorUrl decoding failed with: {:?}",
                    err
                );
                Self::from_nrsurl(url)
            }
        }
    }

    /// Parses a safe nrsurl into SafeUrl
    ///
    /// # Arguments
    ///
    /// * `nrsurl` - an nrsurl.
    pub fn from_nrsurl(nrsurl: &str) -> Result<Self> {
        let parts = SafeUrlParts::parse(&nrsurl)?;

        let hashed_name = Self::xor_name_from_nrs_string(&parts.top_name)?;

        let x = Self::new(
            hashed_name,
            Some(&parts.public_name),
            NRS_MAP_TYPE_TAG,
            SafeDataType::PublishedSeqAppendOnlyData,
            SafeContentType::NrsMapContainer,
            Some(&parts.path),
            Some(parts.sub_names_vec),
            Some(&parts.query_string),
            Some(&parts.fragment),
            None,
        )?;

        Ok(x)
    }

    /// Parses a safe xorurl into SafeUrl
    ///
    /// # Arguments
    ///
    /// * `xorurl` - an xorurl.
    pub fn from_xorurl(xorurl: &str) -> Result<Self> {
        let parts = SafeUrlParts::parse(&xorurl)?;

        let (_base, xorurl_bytes): (Base, Vec<u8>) = decode(&parts.top_name)
            .map_err(|err| Error::InvalidXorUrl(format!("Failed to decode XOR-URL: {:?}", err)))?;

        let type_tag_offset = XOR_NAME_BYTES_OFFSET + XOR_NAME_LEN; // offset where to find the type tag bytes

        // check if too short
        if xorurl_bytes.len() < type_tag_offset {
            return Err(Error::InvalidXorUrl(format!(
                "Invalid XOR-URL, encoded string too short: {} bytes",
                xorurl_bytes.len()
            )));
        }

        // check if too long
        if xorurl_bytes.len() > XOR_URL_STR_MAX_LENGTH {
            return Err(Error::InvalidXorUrl(format!(
                "Invalid XOR-URL, encoded string too long: {} bytes",
                xorurl_bytes.len()
            )));
        }

        // let's make sure we support the XOR_URL version
        let u8_version: u8 = xorurl_bytes[0];
        let encoding_version: u64 = u64::from(u8_version);
        if encoding_version != XOR_URL_VERSION_1 {
            return Err(Error::InvalidXorUrl(format!(
                "Invalid or unsupported XOR-URL encoding version: {}",
                encoding_version
            )));
        }

        let mut content_type_bytes = [0; 2];
        content_type_bytes[0..].copy_from_slice(&xorurl_bytes[1..3]);
        let content_type = match u16::from_be_bytes(content_type_bytes) {
            0 => SafeContentType::Raw,
            1 => SafeContentType::Wallet,
            2 => SafeContentType::FilesContainer,
            3 => SafeContentType::NrsMapContainer,
            other => match MEDIA_TYPE_STR.get(&other) {
                Some(media_type_str) => SafeContentType::MediaType((*media_type_str).to_string()),
                None => {
                    return Err(Error::InvalidXorUrl(format!(
                        "Invalid content type encoded in the XOR-URL string: {}",
                        other
                    )))
                }
            },
        };

        debug!(
            "Attempting to match content type of URL: {}, {:?}",
            &xorurl, content_type
        );

        let data_type = match xorurl_bytes[3] {
            0 => SafeDataType::SafeKey,
            1 => SafeDataType::PublishedImmutableData,
            2 => SafeDataType::UnpublishedImmutableData,
            3 => SafeDataType::SeqMutableData,
            4 => SafeDataType::UnseqMutableData,
            5 => SafeDataType::PublishedSeqAppendOnlyData,
            6 => SafeDataType::PublishedUnseqAppendOnlyData,
            7 => SafeDataType::UnpublishedSeqAppendOnlyData,
            8 => SafeDataType::UnpublishedUnseqAppendOnlyData,
            other => {
                return Err(Error::InvalidXorUrl(format!(
                    "Invalid SAFE data type encoded in the XOR-URL string: {}",
                    other
                )))
            }
        };

        let mut xor_name = XorName::default();
        xor_name
            .0
            .copy_from_slice(&xorurl_bytes[XOR_NAME_BYTES_OFFSET..type_tag_offset]);

        let type_tag_bytes_len = xorurl_bytes.len() - type_tag_offset;

        let mut type_tag_bytes = [0; 8];
        type_tag_bytes[8 - type_tag_bytes_len..].copy_from_slice(&xorurl_bytes[type_tag_offset..]);
        let type_tag: u64 = u64::from_be_bytes(type_tag_bytes);

        let x = Self::new(
            xor_name,
            None, // no nrs_name for an xorurl
            type_tag,
            data_type,
            content_type,
            Some(&parts.path),
            Some(parts.sub_names_vec),
            Some(&parts.query_string),
            Some(&parts.fragment),
            None,
        )?;

        Ok(x)
    }

    /// The url scheme.  Only 'safe' scheme is presently supported.
    pub fn scheme(&self) -> &str {
        SAFE_URL_SCHEME
    }

    /// returns encoding version of xorurl
    pub fn encoding_version(&self) -> u64 {
        self.encoding_version
    }

    /// returns SAFE data type
    pub fn data_type(&self) -> SafeDataType {
        self.data_type.clone()
    }

    /// returns SAFE content type
    pub fn content_type(&self) -> SafeContentType {
        self.content_type.clone()
    }

    /// returns XorName
    pub fn xorname(&self) -> XorName {
        self.xor_name
    }

    /// returns public_name portion of xorurl using the
    /// default xorurl encoding.
    ///
    /// public_name means sub_names + top_name
    ///
    /// useful for retrieving xorurl name associated
    /// with an nrsurl.
    ///
    /// For a different encoding, see name_to_base()
    pub fn xorurl_public_name(&self) -> String {
        self.name_to_base(DEFAULT_XORURL_BASE, true)
    }

    /// The public_name in url.  Either nrs_name or xor_name.
    ///
    /// eg a.b.name --> a.b.name
    pub fn public_name(&self) -> &str {
        &self.public_name
    }

    /// returns top name of name field.
    ///
    /// eg: a.b.name --> name
    pub fn top_name(&self) -> &str {
        &self.top_name
    }

    /// returns sub_names
    ///
    /// eg: a.b.name --> a.b
    pub fn sub_names(&self) -> &str {
        &self.sub_names
    }

    /// returns sub_names in an array slice
    ///
    /// eg: a.b.name --> &["a", "b"]
    pub fn sub_names_vec(&self) -> &[String] {
        &self.sub_names_vec
    }

    /// returns XorUrl type tag
    pub fn type_tag(&self) -> u64 {
        self.type_tag
    }

    /// returns path portion of URL, percent encoded (unmodified).
    pub fn path(&self) -> &str {
        &self.path
    }

    /// returns path portion of URL, percent decoded
    pub fn path_decoded(&self) -> Result<String> {
        Self::url_percent_decode(&self.path)
    }

    /// sets path portion of URL
    ///
    /// input string must not be percent-encoded.
    /// The encoding is done internally.
    ///
    /// leading slash is automatically added if necessary.
    pub fn set_path(&mut self, path: &str) {
        self.set_path_internal(path, true);
    }

    /// gets content version
    ///
    /// This is a shortcut method for getting the "?v=" query param.
    pub fn content_version(&self) -> Option<u64> {
        self.content_version
    }

    /// sets content version
    ///
    /// This is a shortcut method for setting the "?v=" query param.
    ///
    /// # Arguments
    ///
    /// * `version` - u64 representing value of ?v=<val>
    pub fn set_content_version(&mut self, version: Option<u64>) {
        // Convert Option<u64> to Option<&str>
        let version_string: String;
        let v_option = match version {
            Some(v) => {
                version_string = v.to_string();
                Some(version_string.as_str())
            }
            None => None,
        };

        // note: We are being passed a u64
        // which logically should never fail to be set.  Details of
        // this implementation presently require parsing the query
        // string, but that could change in the future without API changing.
        // eg: by storing parsed key/val pairs.
        // Parsing of the query string is checked/validated by
        // set_query_string().  Thus, it should never be invalid, else
        // we have a serious bug in SafeUrl impl.
        self.set_query_key(URL_VERSION_QUERY_NAME, v_option)
            .unwrap_or_else(|e| {
                warn!("{}", e);
            });
    }

    /// sets or unsets a key/val pair in query string.
    ///
    /// if val is Some, then key=val will be set in query string.
    ///    If there is more than one instance of key in query string,
    ///    there will be only one after this call.
    /// If val is None, then the key will be removed from query string.
    ///
    /// To set key without any value, pass Some<""> as the val.
    ///
    /// `val` should not be percent-encoded.  That is done internally.
    ///
    /// # Arguments
    ///
    /// * `key` - name of url query string var
    /// * `val` - an option representing the value, or none.
    pub fn set_query_key(&mut self, key: &str, val: Option<&str>) -> Result<()> {
        let mut pairs = url::form_urlencoded::Serializer::new(String::new());
        let url = Self::query_string_to_url(&self.query_string)?;
        let mut set_key = false;
        for (k, v) in url.query_pairs() {
            if k == key {
                // note: this will consolidate multiple ?k= into just one.
                if let Some(v) = val {
                    if !set_key {
                        pairs.append_pair(key, v);
                        set_key = true;
                    }
                }
            } else {
                pairs.append_pair(&k, &v);
            }
        }
        if !set_key {
            if let Some(v) = val {
                pairs.append_pair(key, v);
            }
        }

        self.query_string = pairs.finish();
        debug!("Set query_string: {}", self.query_string);

        if key == URL_VERSION_QUERY_NAME {
            self.set_content_version_internal(val)?;
        }
        Ok(())
    }

    /// sets query string.
    ///
    /// If the query string contains ?v=<version> then it
    /// will take effect as if set_content_version() had been
    /// called.
    ///
    /// # Arguments
    ///
    /// * `query` - percent-encoded key/val pairs.
    pub fn set_query_string(&mut self, query: &str) -> Result<()> {
        // ?v is a special case, so if it is contained in query string
        // we parse it and update our stored content_version.
        // tbd: another option could be to throw an error if input
        // contains ?v.
        let v_option = Self::query_key_last_internal(query, URL_VERSION_QUERY_NAME);
        self.set_content_version_internal(v_option.as_deref())?;

        self.query_string = query.to_string();
        Ok(())
    }

    /// Retrieves query string
    ///
    /// This contains the percent-encoded key/value pairs
    /// as seen in a url.
    pub fn query_string(&self) -> &str {
        &self.query_string
    }

    /// Retrieves query string, with ? separator if non-empty.
    pub fn query_string_with_separator(&self) -> String {
        let qs = self.query_string();
        if qs.is_empty() {
            qs.to_string()
        } else {
            format!("?{}", qs)
        }
    }

    /// Retrieves all query pairs, percent-decoded.
    pub fn query_pairs(&self) -> Vec<(String, String)> {
        Self::query_pairs_internal(&self.query_string)
    }

    /// Queries a key from the query string.
    ///
    /// Can return 0, 1, or many values because a given key
    /// may exist 0, 1, or many times in a URL query-string.
    pub fn query_key(&self, key: &str) -> Vec<String> {
        Self::query_key_internal(&self.query_string, key)
    }

    /// returns the last matching key from a query string.
    ///
    /// eg in safe://name?color=red&age=5&color=green&color=blue
    ///    blue would be returned when key is "color".
    pub fn query_key_last(&self, key: &str) -> Option<String> {
        Self::query_key_last_internal(&self.query_string, key)
    }

    /// returns the first matching key from a query string.
    ///
    /// eg in safe://name?color=red&age=5&color=green&color=blue
    ///    red would be returned when key is "color".
    pub fn query_key_first(&self, key: &str) -> Option<String> {
        Self::query_key_first_internal(&self.query_string, key)
    }

    /// sets url fragment
    pub fn set_fragment(&mut self, fragment: String) {
        self.fragment = fragment;
    }

    /// Retrieves url fragment, without # separator
    pub fn fragment(&self) -> &str {
        &self.fragment
    }

    /// Retrieves url fragment, with # separator if non-empty.
    pub fn fragment_with_separator(&self) -> String {
        if self.fragment.is_empty() {
            "".to_string()
        } else {
            format!("#{}", self.fragment)
        }
    }

    /// returns true if an NrsUrl, false if an XorUrl
    pub fn is_nrsurl(&self) -> bool {
        self.safeurl_type == SafeUrlType::NrsUrl
    }

    /// returns true if an XorUrl, false if an NrsUrl
    pub fn is_xorurl(&self) -> bool {
        self.safeurl_type == SafeUrlType::XorUrl
    }

    /// returns type of this SafeUrl.
    ///
    /// for type of the linked content, see
    ///   ::content_type()
    pub fn safeurl_type(&self) -> &SafeUrlType {
        &self.safeurl_type
    }

    // XOR-URL encoding format (var length from 36 to 44 bytes):
    // 1 byte for encoding version
    // 2 bytes for content type (enough to start including some MIME types also)
    // 1 byte for SAFE native data type
    // 32 bytes for XoR Name
    // and up to 8 bytes for type_tag
    // query param "v=" is treated as the content version

    /// serializes the URL to an XorUrl string.
    ///
    /// This function may be called on an NrsUrl and
    /// the corresponding XorUrl will be returned.
    pub fn to_xorurl_string(&self) -> String {
        self.to_base(DEFAULT_XORURL_BASE)
    }

    /// serializes the URL to an NrsUrl string.
    ///
    /// This function returns None when is_nrsurl() is false.
    pub fn to_nrsurl_string(&self) -> Option<String> {
        if !self.is_nrsurl() {
            return None;
        }

        let query_string = self.query_string_with_separator();
        let fragment = self.fragment_with_separator();

        let url = format!(
            "{}{}{}{}{}",
            SAFE_URL_PROTOCOL, self.public_name, self.path, query_string, fragment
        );
        Some(url)
    }

    /// serializes entire xorurl using a particular base encoding.
    pub fn to_base(&self, base: XorUrlBase) -> String {
        let name = self.name_to_base(base, true);

        let query_string = self.query_string_with_separator();
        let fragment = self.fragment_with_separator();

        // serialize full xorurl
        format!(
            "{}{}{}{}{}",
            SAFE_URL_PROTOCOL, name, self.path, query_string, fragment
        )
    }

    /// serializes name portion of xorurl using a particular base encoding.
    pub fn name_to_base(&self, base: XorUrlBase, include_subnames: bool) -> String {
        // let's set the first byte with the XOR-URL format version
        let mut cid_vec: Vec<u8> = vec![XOR_URL_VERSION_1 as u8];

        cid_vec.extend_from_slice(&self.content_type_u16.to_be_bytes());

        // push the SAFE data type byte
        cid_vec.push(self.data_type.clone() as u8);

        // add the xor_name 32 bytes
        cid_vec.extend_from_slice(&self.xor_name.0);

        // let's get non-zero bytes only from th type_tag
        let start_byte: usize = (self.type_tag.leading_zeros() / 8) as usize;
        // add the non-zero bytes of type_tag
        cid_vec.extend_from_slice(&self.type_tag.to_be_bytes()[start_byte..]);

        let base_encoding = match base {
            XorUrlBase::Base32z => Base::Base32z,
            XorUrlBase::Base32 => Base::Base32,
            XorUrlBase::Base64 => Base::Base64,
        };
        let top_name = encode(base_encoding, cid_vec);

        if include_subnames {
            let sub_names = self.sub_names();
            let sep = if sub_names.is_empty() { "" } else { "." };
            format!("{}{}{}", sub_names, sep, top_name)
        } else {
            top_name
        }
    }

    /// Utility function to perform url percent decoding.
    pub fn url_percent_decode(s: &str) -> Result<String> {
        match urlencoding::decode(s) {
            Ok(c) => Ok(c),
            Err(e) => Err(Error::InvalidInput(format!("{:#?}", e))),
        }
    }

    /// Utility function to perform url percent encoding.
    pub fn url_percent_encode(s: &str) -> String {
        urlencoding::encode(s)
    }

    /// Validates that a SafeUrl instance can be parsed correctly.
    ///
    /// SafeUrl::from_url() performs rigorous validation,
    /// however setters and new() do not enforce all the rules
    ///
    /// This routine enables a caller to easily validate
    /// that the present instance passes all validation checks
    pub fn validate(&self) -> Result<()> {
        let s = self.to_string();
        match Self::from_url(&s) {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    // A non-member encoder function for convenience in some cases
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        xor_name: XorName,
        nrs_name: Option<&str>,
        type_tag: u64,
        data_type: SafeDataType,
        content_type: SafeContentType,
        path: Option<&str>,
        sub_names: Option<Vec<String>>,
        query_string: Option<&str>,
        fragment: Option<&str>,
        content_version: Option<u64>,
        base: XorUrlBase,
    ) -> Result<String> {
        let xorurl_encoder = SafeUrl::new(
            xor_name,
            nrs_name,
            type_tag,
            data_type,
            content_type,
            path,
            sub_names,
            query_string,
            fragment,
            content_version,
        )?;
        Ok(xorurl_encoder.to_base(base))
    }

    // A non-member SafeKey encoder function for convenience
    pub fn encode_safekey(xor_name: XorName, base: XorUrlBase) -> Result<String> {
        SafeUrl::encode(
            xor_name,
            None,
            0,
            SafeDataType::SafeKey,
            SafeContentType::Raw,
            None,
            None,
            None,
            None,
            None,
            base,
        )
    }

    // A non-member ImmutableData encoder function for convenience
    pub fn encode_immutable_data(
        xor_name: XorName,
        content_type: SafeContentType,
        base: XorUrlBase,
    ) -> Result<String> {
        SafeUrl::encode(
            xor_name,
            None,
            0,
            SafeDataType::PublishedImmutableData,
            content_type,
            None,
            None,
            None,
            None,
            None,
            base,
        )
    }

    // A non-member MutableData encoder function for convenience
    pub fn encode_mutable_data(
        xor_name: XorName,
        type_tag: u64,
        content_type: SafeContentType,
        base: XorUrlBase,
    ) -> Result<String> {
        SafeUrl::encode(
            xor_name,
            None,
            type_tag,
            SafeDataType::SeqMutableData,
            content_type,
            None,
            None,
            None,
            None,
            None,
            base,
        )
    }

    // A non-member AppendOnlyData encoder function for convenience
    pub fn encode_append_only_data(
        xor_name: XorName,
        type_tag: u64,
        content_type: SafeContentType,
        base: XorUrlBase,
    ) -> Result<String> {
        SafeUrl::encode(
            xor_name,
            None,
            type_tag,
            SafeDataType::PublishedSeqAppendOnlyData,
            content_type,
            None,
            None,
            None,
            None,
            None,
            base,
        )
    }

    // utility to generate a dummy url from a query string.
    fn query_string_to_url(query_str: &str) -> Result<Url> {
        let dummy = format!("file://dummy?{}", query_str);
        match Url::parse(&dummy) {
            Ok(u) => Ok(u),
            Err(_e) => {
                let msg = format!("Invalid query string: {}", query_str);
                Err(Error::InvalidInput(msg))
            }
        }
    }

    // utility to retrieve all unescaped key/val pairs from query string.
    //
    // note: It's not that efficient parsing query-string for each
    // get_key request, but the alternative would be storing pairs
    // in the struct when query string is set, which takes extra
    // space.  and since query-string is mostly opaque to us anyway,
    // I figure this is adequate for now, but might want to revisit
    // later if perf ever becomes important, eg for Client Apps.
    // API needn't change.
    fn query_pairs_internal(query_str: &str) -> Vec<(String, String)> {
        let url = match Self::query_string_to_url(query_str) {
            Ok(u) => u,
            Err(_) => {
                return Vec::<(String, String)>::new();
            }
        };

        let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
        pairs
    }

    // sets content_version property.
    //
    // This should never be called directly.
    // Use ::set_content_version() or ::set_query_key() instead.
    fn set_content_version_internal(&mut self, version_option: Option<&str>) -> Result<()> {
        if let Some(version_str) = version_option {
            let version = version_str.parse::<u64>().map_err(|_e| {
                let msg = format!(
                    "{} param could not be parsed as u64. invalid: '{}'",
                    URL_VERSION_QUERY_NAME, version_str
                );
                Error::InvalidInput(msg)
            })?;
            self.content_version = Some(version);
        } else {
            self.content_version = None;
        }
        debug!("Set version: {:#?}", self.content_version);
        Ok(())
    }

    // sets path portion of URL
    //
    // input path may be percent-encoded or not, but
    // percent_encode param must be set appropriately
    // to avoid not-encoded or double-encoded isues.
    //
    // leading slash is automatically added if necessary.
    fn set_path_internal(&mut self, path: &str, percent_encode: bool) {
        // fast path for empty string.
        if path.is_empty() {
            if !self.path.is_empty() {
                self.path = path.to_string();
            }
            return;
        }

        // impl note: this func tries to behave like url::Url::set_path()
        // with respect to percent-encoding each path component.
        //
        // tbd: It might be more correct to simply instantiate a
        // dummy URL and call set_path(), return path();
        // counter-argument is that Url::set_path() does not
        // prefix leading slash and allows urls to be created
        // that merge name and path together.
        let parts: Vec<&str> = path.split('/').collect();
        let mut new_parts = Vec::<String>::new();
        for (count, p) in parts.into_iter().enumerate() {
            if !p.is_empty() || count > 0 {
                if percent_encode {
                    new_parts.push(Self::url_percent_encode(p));
                } else {
                    new_parts.push(p.to_string());
                }
            }
        }
        let new_path = new_parts.join("/");

        let separator = if new_path.is_empty() { "" } else { "/" };
        self.path = format!("{}{}", separator, new_path);
    }

    // utility to query a key from a query string, percent-decoded.
    // Can return 0, 1, or many values because a given key
    // can exist 0, 1, or many times in a URL query-string.
    fn query_key_internal(query_str: &str, key: &str) -> Vec<String> {
        let pairs = Self::query_pairs_internal(query_str);
        let mut values = Vec::<String>::new();

        for (k, val) in pairs {
            if k == key {
                values.push(val);
            }
        }
        values
    }

    // utility to query a key from a query string, percent-decoded.
    // returns the last matching key.
    // eg in safe://name?color=red&age=5&color=green&color=blue
    //    blue would be returned when key is "color".
    fn query_key_last_internal(query_str: &str, key: &str) -> Option<String> {
        let matches = Self::query_key_internal(query_str, key);
        match matches.last() {
            Some(v) => Some(v.to_string()),
            None => None,
        }
    }

    // utility to query a key from a query string.
    // returns the last matching key.
    // eg in safe://name?color=red&age=5&color=green&color=blue
    //    blue would be returned when key is "color".
    fn query_key_first_internal(query_str: &str, key: &str) -> Option<String> {
        let matches = Self::query_key_internal(query_str, key);
        match matches.first() {
            Some(v) => Some(v.to_string()),
            None => None,
        }
    }

    fn xor_name_from_nrs_string(name: &str) -> Result<XorName> {
        let vec_hash = sha3_256(&name.to_string().into_bytes());
        let xor_name = XorName(vec_hash);
        debug!("Resulting XorName for NRS \"{}\" is: {}", name, xor_name);
        Ok(xor_name)
    }
}

impl fmt::Display for SafeUrl {
    /// serializes the URL to a string.
    ///
    /// an NrsUrl will be serialized in NrsUrl form.
    /// an XorUrl will be serialized in XorUrl form.
    ///
    /// See also:
    ///  * ::to_xorurl_string()
    ///  * ::to_nrs_url_string()
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        let buf = if self.is_nrsurl() {
            match self.to_nrsurl_string() {
                Some(s) => s,
                None => {
                    warn!("to_nrsurl_string() return None when is_nrsurl() == true. '{}'.  This should never happen. Please investigate.", self.public_name);
                    return Err(fmt::Error);
                }
            }
        } else {
            self.to_xorurl_string()
        };
        write!(fmt, "{}", buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safeurl_new_validation() -> Result<()> {
        // Tests some errors when calling Self::new()

        let msg = "Expected error";
        let wrong_err = "Wrong error type";

        let xor_name = XorName(*b"12345678901234567890123456789012");

        // test: "Media-type '{}' not supported. You can use 'SafeContentType::Raw' as the 'content_type' for this type of content",
        let result = SafeUrl::new(
            xor_name,
            None,
            NRS_MAP_TYPE_TAG,
            SafeDataType::PublishedSeqAppendOnlyData,
            SafeContentType::MediaType("garbage/trash".to_string()),
            None,
            None,
            None,
            None,
            None,
        )
        .expect_err(msg);
        match result {
            Error::InvalidMediaType(e) => assert!(e.contains("You can use 'SafeContentType::Raw'")),
            _ => {
                return Err(Error::Unexpected(wrong_err.to_string()));
            }
        }

        // test: "nrs_name cannot be empty string."
        let result = SafeUrl::new(
            xor_name,
            Some(""), // passing empty string as nrs name
            NRS_MAP_TYPE_TAG,
            SafeDataType::PublishedSeqAppendOnlyData,
            SafeContentType::NrsMapContainer,
            None,
            None,
            None,
            None,
            None,
        )
        .expect_err(msg);
        match result {
            Error::InvalidInput(e) => assert!(e.contains("nrs_name cannot be empty string.")),
            _ => panic!(wrong_err),
        }

        // test: "input mis-match. nrs_name `{}` does not hash to xor_name `{}`"
        let result = SafeUrl::new(
            xor_name,
            Some("a.b.c"), // passing nrs name not matching xor_name.
            NRS_MAP_TYPE_TAG,
            SafeDataType::PublishedSeqAppendOnlyData,
            SafeContentType::NrsMapContainer,
            None,
            None,
            None,
            None,
            None,
        )
        .expect_err(msg);
        match result {
            Error::InvalidInput(e) => assert!(e.contains("does not hash to xor_name")),
            _ => panic!(wrong_err),
        }

        // test: "Host contains empty subname" (in nrs name)
        let result = SafeUrl::new(
            xor_name,
            Some("a..b.c"), // passing empty sub-name in nrs name
            NRS_MAP_TYPE_TAG,
            SafeDataType::PublishedSeqAppendOnlyData,
            SafeContentType::NrsMapContainer,
            None,
            None,
            None,
            None,
            None,
        )
        .expect_err(msg);
        match result {
            Error::InvalidXorUrl(e) => assert!(e.contains("name contains empty subname")),
            _ => panic!(wrong_err),
        }

        // test: "empty subname" (in xorurl sub_names)
        let result = SafeUrl::new(
            xor_name,
            None, // not NRS
            NRS_MAP_TYPE_TAG,
            SafeDataType::PublishedSeqAppendOnlyData,
            SafeContentType::NrsMapContainer,
            None,
            Some(vec!["a".to_string(), "".to_string(), "b".to_string()]),
            None,
            None,
            None,
        )
        .expect_err(msg);
        match result {
            Error::InvalidInput(e) => assert!(e.contains("empty subname")),
            _ => panic!(wrong_err),
        }

        Ok(())
    }

    #[test]
    fn test_safeurl_base32_encoding() -> Result<()> {
        let xor_name = XorName(*b"12345678901234567890123456789012");
        let xorurl = SafeUrl::encode(
            xor_name,
            None,
            0xa632_3c4d_4a32,
            SafeDataType::PublishedImmutableData,
            SafeContentType::Raw,
            None,
            None,
            None,
            None,
            None,
            XorUrlBase::Base32,
        )?;
        let base32_xorurl =
            "safe://biaaaatcmrtgq2tmnzyheydcmrtgq2tmnzyheydcmrtgq2tmnzyheydcmvggi6e2srs";
        assert_eq!(xorurl, base32_xorurl);
        Ok(())
    }

    #[test]
    fn test_safeurl_base32z_encoding() -> Result<()> {
        let xor_name = XorName(*b"12345678901234567890123456789012");
        let xorurl =
            SafeUrl::encode_immutable_data(xor_name, SafeContentType::Raw, XorUrlBase::Base32z)?;
        let base32z_xorurl = "safe://hbyyyyncj1gc4dkptz8yhuycj1gc4dkptz8yhuycj1gc4dkptz8yhuycj1";
        assert_eq!(xorurl, base32z_xorurl);
        Ok(())
    }

    #[test]
    fn test_safeurl_base64_encoding() -> Result<()> {
        let xor_name = XorName(*b"12345678901234567890123456789012");
        let xorurl = SafeUrl::encode_append_only_data(
            xor_name,
            4_584_545,
            SafeContentType::FilesContainer,
            XorUrlBase::Base64,
        )?;
        let base64_xorurl = "safe://mQACBTEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNDU2Nzg5MDEyRfRh";
        assert_eq!(xorurl, base64_xorurl);
        let xorurl_encoder = SafeUrl::from_url(&base64_xorurl)?;
        assert_eq!(base64_xorurl, xorurl_encoder.to_base(XorUrlBase::Base64));
        assert_eq!("", xorurl_encoder.path());
        assert_eq!(XOR_URL_VERSION_1, xorurl_encoder.encoding_version());
        assert_eq!(xor_name, xorurl_encoder.xorname());
        assert_eq!(4_584_545, xorurl_encoder.type_tag());
        assert_eq!(
            SafeDataType::PublishedSeqAppendOnlyData,
            xorurl_encoder.data_type()
        );
        assert_eq!(
            SafeContentType::FilesContainer,
            xorurl_encoder.content_type()
        );
        Ok(())
    }

    #[test]
    fn test_safeurl_default_base_encoding() -> Result<()> {
        let xor_name = XorName(*b"12345678901234567890123456789012");
        let base32z_xorurl = "safe://hbyyyyncj1gc4dkptz8yhuycj1gc4dkptz8yhuycj1gc4dkptz8yhuycj1";
        let xorurl =
            SafeUrl::encode_immutable_data(xor_name, SafeContentType::Raw, DEFAULT_XORURL_BASE)?;
        assert_eq!(xorurl, base32z_xorurl);
        Ok(())
    }

    #[test]
    fn test_safeurl_decoding() -> Result<()> {
        let xor_name = XorName(*b"12345678901234567890123456789012");
        let type_tag: u64 = 0x0eef;
        let subdirs = "/dir1/dir2";
        let content_version = 5;
        let query_string = "k1=v1&k2=v2";
        let query_string_v = format!("{}&v={}", query_string, content_version);
        let fragment = "myfragment";
        let xorurl = SafeUrl::encode(
            xor_name,
            None,
            type_tag,
            SafeDataType::PublishedImmutableData,
            SafeContentType::Raw,
            Some(subdirs),
            Some(vec!["subname".to_string()]),
            Some(query_string),
            Some(fragment),
            Some(5),
            XorUrlBase::Base32z,
        )?;
        let xorurl_encoder = SafeUrl::from_url(&xorurl)?;

        assert_eq!(subdirs, xorurl_encoder.path());
        assert_eq!(XOR_URL_VERSION_1, xorurl_encoder.encoding_version());
        assert_eq!(xor_name, xorurl_encoder.xorname());
        assert_eq!(type_tag, xorurl_encoder.type_tag());
        assert_eq!(
            SafeDataType::PublishedImmutableData,
            xorurl_encoder.data_type()
        );
        assert_eq!(SafeContentType::Raw, xorurl_encoder.content_type());
        assert_eq!(Some(content_version), xorurl_encoder.content_version());
        assert_eq!(query_string_v, xorurl_encoder.query_string());
        assert_eq!(fragment, xorurl_encoder.fragment());
        Ok(())
    }

    #[test]
    fn test_safeurl_decoding_with_path() -> Result<()> {
        let xor_name = XorName(*b"12345678901234567890123456789012");
        let type_tag: u64 = 0x0eef;
        let xorurl = SafeUrl::encode_append_only_data(
            xor_name,
            type_tag,
            SafeContentType::Wallet,
            XorUrlBase::Base32z,
        )?;

        let xorurl_with_path = format!("{}/subfolder/file", xorurl);
        let xorurl_encoder_with_path = SafeUrl::from_url(&xorurl_with_path)?;
        assert_eq!(
            xorurl_with_path,
            xorurl_encoder_with_path.to_base(XorUrlBase::Base32z)
        );
        assert_eq!("/subfolder/file", xorurl_encoder_with_path.path());
        assert_eq!(
            XOR_URL_VERSION_1,
            xorurl_encoder_with_path.encoding_version()
        );
        assert_eq!(xor_name, xorurl_encoder_with_path.xorname());
        assert_eq!(type_tag, xorurl_encoder_with_path.type_tag());
        assert_eq!(
            SafeDataType::PublishedSeqAppendOnlyData,
            xorurl_encoder_with_path.data_type()
        );
        assert_eq!(
            SafeContentType::Wallet,
            xorurl_encoder_with_path.content_type()
        );
        Ok(())
    }

    #[test]
    fn test_safeurl_decoding_with_subname() -> Result<()> {
        let xor_name = XorName(*b"12345678901234567890123456789012");
        let type_tag: u64 = 0x0eef;
        let xorurl_with_subname = SafeUrl::encode(
            xor_name,
            None,
            type_tag,
            SafeDataType::PublishedImmutableData,
            SafeContentType::NrsMapContainer,
            None,
            Some(vec!["sub".to_string()]),
            None,
            None,
            None,
            XorUrlBase::Base32z,
        )?;

        assert!(xorurl_with_subname.contains("safe://sub."));
        let xorurl_encoder_with_subname = SafeUrl::from_url(&xorurl_with_subname)?;
        assert_eq!(
            xorurl_with_subname,
            xorurl_encoder_with_subname.to_base(XorUrlBase::Base32z)
        );
        assert_eq!("", xorurl_encoder_with_subname.path());
        assert_eq!(1, xorurl_encoder_with_subname.encoding_version());
        assert_eq!(xor_name, xorurl_encoder_with_subname.xorname());
        assert_eq!(type_tag, xorurl_encoder_with_subname.type_tag());
        assert_eq!(&["sub"], xorurl_encoder_with_subname.sub_names_vec());
        assert_eq!(
            SafeContentType::NrsMapContainer,
            xorurl_encoder_with_subname.content_type()
        );
        Ok(())
    }

    #[test]
    fn test_safeurl_encoding_decoding_with_media_type() -> Result<()> {
        let xor_name = XorName(*b"12345678901234567890123456789012");
        let xorurl = SafeUrl::encode_immutable_data(
            xor_name,
            SafeContentType::MediaType("text/html".to_string()),
            XorUrlBase::Base32z,
        )?;

        let xorurl_encoder = SafeUrl::from_url(&xorurl)?;
        assert_eq!(
            SafeContentType::MediaType("text/html".to_string()),
            xorurl_encoder.content_type()
        );
        Ok(())
    }

    #[test]
    fn test_safeurl_too_long() -> Result<()> {
        let xorurl =
            "safe://heyyynunctugo4ucp3a8radnctugo4ucp3a8radnctugo4ucp3a8radnctmfp5zq75zq75zq7";

        match SafeUrl::from_xorurl(xorurl) {
            Ok(_) => Err(Error::Unexpected(
                "Unexpectedly parsed an invalid (too long) xorurl".to_string(),
            )),
            Err(Error::InvalidXorUrl(msg)) => {
                assert!(msg.starts_with("Invalid XOR-URL, encoded string too long"));
                Ok(())
            }
            other => Err(Error::Unexpected(format!(
                "Error returned is not the expected one: {:?}",
                other
            ))),
        }
    }

    #[test]
    fn test_safeurl_too_short() -> Result<()> {
        let xor_name = XorName(*b"12345678901234567890123456789012");
        let xorurl = SafeUrl::encode_immutable_data(
            xor_name,
            SafeContentType::MediaType("text/html".to_string()),
            XorUrlBase::Base32z,
        )?;

        let len = xorurl.len() - 1;
        match SafeUrl::from_xorurl(&xorurl[..len]) {
            Ok(_) => Err(Error::Unexpected(
                "Unexpectedly parsed an invalid (too short) xorurl".to_string(),
            )),
            Err(Error::InvalidXorUrl(msg)) => {
                assert!(msg.starts_with("Invalid XOR-URL, encoded string too short"));
                Ok(())
            }
            other => Err(Error::Unexpected(format!(
                "Error returned is not the expected one: {:?}",
                other
            ))),
        }
    }

    #[test]
    fn test_safeurl_query_key_first() -> Result<()> {
        let x = SafeUrl::from_url("safe://myname?name=John+Doe&name=Jane%20Doe")?;
        let name = x.query_key_first("name");
        assert_eq!(name, Some("John Doe".to_string()));

        Ok(())
    }

    #[test]
    fn test_safeurl_query_key_last() -> Result<()> {
        let x = SafeUrl::from_url("safe://myname?name=John+Doe&name=Jane%20Doe")?;
        let name = x.query_key_last("name");
        assert_eq!(name, Some("Jane Doe".to_string()));

        Ok(())
    }

    #[test]
    fn test_safeurl_query_key() -> Result<()> {
        let x = SafeUrl::from_url("safe://myname?name=John+Doe&name=Jane%20Doe")?;
        let name = x.query_key("name");
        assert_eq!(name, vec!["John Doe".to_string(), "Jane Doe".to_string()]);

        Ok(())
    }

    #[test]
    fn test_safeurl_set_query_key() -> Result<()> {
        let mut x = SafeUrl::from_url("safe://myname?name=John+Doe&name=Jane%20Doe")?;

        // set_query_key should replace the multiple name= with a single instance.
        let peggy_sue = "Peggy Sue".to_string();
        x.set_query_key("name", Some(&peggy_sue))?;
        assert_eq!(x.query_key_first("name"), Some(peggy_sue.clone()));
        assert_eq!(x.query_key_last("name"), Some(peggy_sue));
        assert_eq!(x.to_string(), "safe://myname?name=Peggy+Sue");

        // None should remove the name param.
        x.set_query_key("name", None)?;
        assert_eq!(x.query_key_last("name"), None);
        assert_eq!(x.to_string(), "safe://myname");

        // Test setting an empty key.
        x.set_query_key("name", Some(""))?;
        x.set_query_key("age", Some("25"))?;
        assert_eq!(x.query_key_last("name"), Some("".to_string()));
        assert_eq!(x.query_key_last("age"), Some("25".to_string()));
        assert_eq!(x.to_string(), "safe://myname?name=&age=25");

        // Test setting content version via ?v=61342
        x.set_query_key(URL_VERSION_QUERY_NAME, Some("61342"))?;
        assert_eq!(
            x.query_key_last(URL_VERSION_QUERY_NAME),
            Some("61342".to_string())
        );
        assert_eq!(x.content_version(), Some(61342));

        // Test unsetting content version via ?v=None
        x.set_query_key(URL_VERSION_QUERY_NAME, None)?;
        assert_eq!(x.query_key_last(URL_VERSION_QUERY_NAME), None);
        assert_eq!(x.content_version(), None);

        // Test parse error for version via ?v=non-integer
        let result = x.set_query_key(URL_VERSION_QUERY_NAME, Some("non-integer"));
        assert!(result.is_err());

        Ok(())
    }

    #[test]
    fn test_safeurl_set_content_version() -> Result<()> {
        let mut x = SafeUrl::from_url("safe://myname?name=John+Doe&name=Jane%20Doe")?;

        x.set_content_version(Some(234));
        assert_eq!(
            x.query_key_first(URL_VERSION_QUERY_NAME),
            Some("234".to_string())
        );
        assert_eq!(x.content_version(), Some(234));
        assert_eq!(
            x.to_string(),
            "safe://myname?name=John+Doe&name=Jane+Doe&v=234"
        );

        x.set_content_version(None);
        assert_eq!(x.query_key_first(URL_VERSION_QUERY_NAME), None);
        assert_eq!(x.content_version(), None);
        assert_eq!(x.to_string(), "safe://myname?name=John+Doe&name=Jane+Doe");

        Ok(())
    }

    #[test]
    fn test_safeurl_path() -> Result<()> {
        // Make sure we can read percent-encoded paths, and set them as well.
        let mut x = SafeUrl::from_url("safe://domain/path/to/my%20file.txt?v=1")?;
        assert_eq!(x.path(), "/path/to/my%20file.txt");
        x.set_path("/path/to/my new file.txt");
        assert_eq!(x.path(), "/path/to/my%20new%20file.txt");
        assert_eq!(x.path_decoded()?, "/path/to/my new file.txt");
        x.set_path("/trailing/slash/");
        assert_eq!(x.path(), "/trailing/slash/");

        // here we verify that url::Url has the same path encoding behavior
        // as our implementation.  for better or worse.
        let mut u = Url::parse("safe://domain/path/to/my%20file.txt?v=1").unwrap();
        assert_eq!(u.path(), "/path/to/my%20file.txt");
        u.set_path("/path/to/my new file.txt");
        assert_eq!(u.path(), "/path/to/my%20new%20file.txt");
        u.set_path("/trailing/slash/");
        assert_eq!(u.path(), "/trailing/slash/");

        // note: our impl and url::Url differ with no-leading-slash behavior.
        // we prepend leading slash when storing and return a changed path.
        // some SAFE code appears to depend on this presently.
        x.set_path("no-leading-slash");
        assert_eq!(x.path(), "/no-leading-slash");
        assert_eq!(x.to_string(), "safe://domain/no-leading-slash?v=1");
        x.set_path("");
        assert_eq!(x.path(), ""); // no slash if path is empty.
        assert_eq!(x.to_string(), "safe://domain?v=1");
        x.set_path("/");
        assert_eq!(x.path(), ""); // slash removed if path otherwise empty.
        assert_eq!(x.to_string(), "safe://domain?v=1");

        // url::Url preserves the missing slash, and allows path to
        // merge with domain.  seems kind of broken.  bug?
        u.set_path("no-leading-slash");
        assert_eq!(u.path(), "no-leading-slash");
        assert_eq!(u.to_string(), "safe://domainno-leading-slash?v=1");
        u.set_path("");
        assert_eq!(u.path(), "");
        assert_eq!(x.to_string(), "safe://domain?v=1");
        u.set_path("/");
        assert_eq!(u.path(), "/");
        assert_eq!(x.to_string(), "safe://domain?v=1"); // note that slash in path omitted.

        Ok(())
    }

    #[test]
    fn test_safeurl_to_string() -> Result<()> {
        // These two are equivalent.  ie, the xorurl is the result of nrs.to_xorurl_string()
        let nrsurl = "safe://my.sub.domain/path/my%20dir/my%20file.txt?this=that&this=other&color=blue&v=5&name=John+Doe#somefragment";
        let xorurl = "safe://my.sub.hnyydyiixsfrqix9aoqg97jebuzc6748uc8rykhdd5hjrtg5o4xso9jmggbqh/path/my%20dir/my%20file.txt?this=that&this=other&color=blue&v=5&name=John+Doe#somefragment";

        let nrs = SafeUrl::from_url(nrsurl)?;
        let xor = SafeUrl::from_url(xorurl)?;

        assert_eq!(nrs.to_string(), nrsurl);
        assert_eq!(xor.to_string(), xorurl);

        assert_eq!(nrs.to_nrsurl_string(), Some(nrsurl.to_string()));
        assert_eq!(nrs.to_xorurl_string(), xorurl);

        assert_eq!(xor.to_nrsurl_string(), None);
        assert_eq!(xor.to_xorurl_string(), xorurl);

        Ok(())
    }

    #[test]
    fn test_safeurl_parts() -> Result<()> {
        // These two are equivalent.  ie, the xorurl is the result of nrs.to_xorurl_string()
        let nrsurl = "safe://my.sub.domain/path/my%20dir/my%20file.txt?this=that&this=other&color=blue&v=5&name=John+Doe#somefragment";
        let xorurl = "safe://my.sub.hnyydyiixsfrqix9aoqg97jebuzc6748uc8rykhdd5hjrtg5o4xso9jmggbqh/path/my%20dir/my%20file.txt?this=that&this=other&color=blue&v=5&name=John+Doe#somefragment";

        let nrs = SafeUrl::from_url(nrsurl)?;
        let xor = SafeUrl::from_url(xorurl)?;

        assert_eq!(nrs.scheme(), SAFE_URL_SCHEME);
        assert_eq!(xor.scheme(), SAFE_URL_SCHEME);

        assert_eq!(nrs.public_name(), "my.sub.domain");
        assert_eq!(
            xor.public_name(),
            "my.sub.hnyydyiixsfrqix9aoqg97jebuzc6748uc8rykhdd5hjrtg5o4xso9jmggbqh"
        );

        assert_eq!(nrs.top_name(), "domain");
        assert_eq!(
            xor.top_name(),
            "hnyydyiixsfrqix9aoqg97jebuzc6748uc8rykhdd5hjrtg5o4xso9jmggbqh"
        );

        assert_eq!(nrs.sub_names(), "my.sub");
        assert_eq!(xor.sub_names(), "my.sub");

        assert_eq!(nrs.sub_names_vec(), ["my", "sub"]);
        assert_eq!(xor.sub_names_vec(), ["my", "sub"]);

        assert_eq!(nrs.path(), "/path/my%20dir/my%20file.txt");
        assert_eq!(xor.path(), "/path/my%20dir/my%20file.txt");

        assert_eq!(nrs.path_decoded()?, "/path/my dir/my file.txt");
        assert_eq!(xor.path_decoded()?, "/path/my dir/my file.txt");

        assert_eq!(
            nrs.query_string(),
            "this=that&this=other&color=blue&v=5&name=John+Doe"
        );
        assert_eq!(
            xor.query_string(),
            "this=that&this=other&color=blue&v=5&name=John+Doe"
        );

        assert_eq!(nrs.fragment(), "somefragment");
        assert_eq!(xor.fragment(), "somefragment");

        Ok(())
    }

    #[test]
    fn test_safeurl_from_url_validation() -> Result<()> {
        // Tests basic URL syntax errors that are common to
        // both ::from_xorurl() and ::from_nrsurl()

        let msg = "Expected error";
        let wrong_err = "Wrong error type";
        let should_work = "should work";

        let result = SafeUrl::from_url("withoutscheme").expect_err(msg);
        match result {
            Error::InvalidXorUrl(e) => assert!(e.contains("relative URL without a base")),
            _ => panic!(wrong_err),
        }

        let result = SafeUrl::from_url("http://badscheme").expect_err(msg);
        match result {
            Error::InvalidXorUrl(e) => assert!(e.contains("invalid scheme")),
            _ => panic!(wrong_err),
        }

        let result = SafeUrl::from_url("safe:///emptyname").expect_err(msg);
        match result {
            Error::InvalidXorUrl(e) => assert!(e.contains("missing name")),
            _ => panic!(wrong_err),
        }

        let result = SafeUrl::from_url("safe://space in name").expect_err(msg);
        match result {
            Error::InvalidXorUrl(e) => assert!(e.contains("invalid domain character")),
            _ => panic!(wrong_err),
        }

        let result = SafeUrl::from_url("safe://my.sub..name").expect_err(msg);
        match result {
            Error::InvalidXorUrl(e) => assert!(e.contains("name contains empty subname")),
            _ => panic!(wrong_err),
        }

        let result = SafeUrl::from_url("safe://name//").expect_err(msg);
        match result {
            Error::InvalidXorUrl(e) => assert!(e.contains("path contains empty component")),
            _ => panic!(wrong_err),
        }

        // note: ?? is actually ok in a standard url.  I suppose no harm in allowing for safe
        // see:  https://stackoverflow.com/questions/2924160/is-it-valid-to-have-more-than-one-question-mark-in-a-url
        let _result = SafeUrl::from_url("safe://name??foo=bar").expect(should_work);

        // note: ## and #frag1#frag2 are accepted by rust URL parser.
        // tbd: if we want to disallow.
        // see: https://stackoverflow.com/questions/10850781/multiple-hash-signs-in-url
        let _result = SafeUrl::from_url("safe://name?foo=bar##fragment").expect(should_work);

        // note: single%percent/in/path is accepted by rust URL parser.
        // tbd: if we want to disallow.
        let _result =
            SafeUrl::from_nrsurl("safe://name/single%percent/in/path").expect(should_work);

        Ok(())
    }

    #[test]
    fn test_safeurl_from_xorurl_validation() -> Result<()> {
        // Tests some URL errors that are specific to xorurl

        let msg = "Expected error";
        let wrong_err = "Wrong error type".to_string();

        // test: "Failed to decode XOR-URL"
        let result = SafeUrl::from_xorurl("safe://invalidxor").expect_err(msg);
        match result {
            Error::InvalidXorUrl(e) => assert!(e.contains("Failed to decode XOR-URL")),
            _ => return Err(Error::Unexpected(wrong_err)),
        }

        // test: xorurl with a space in it
        let result = SafeUrl::from_xorurl(
            "safe://hnyydy iixsfrqix9aoqg97jebuzc6748uc8rykhdd5hjrtg5o4xso9jmggbqh",
        )
        .expect_err(msg);
        println!("{:#?}", result);
        match result {
            Error::InvalidXorUrl(e) => assert!(e.contains("invalid domain character")),
            _ => return Err(Error::Unexpected(wrong_err)),
        }

        // note: too long/short have separate tests already.
        // "Invalid XOR-URL, encoded string too short"
        // "Invalid XOR-URL, encoded string too long"

        // todo: we should have tests for these.  help anyone?
        // "Invalid or unsupported XOR-URL encoding version: {}",
        // "Invalid content type encoded in the XOR-URL string: {}",
        // "Invalid SAFE data type encoded in the XOR-URL string: {}",

        Ok(())
    }

    #[test]
    fn test_safeurl_validate() -> Result<()> {
        let nrsurl = "safe://my.sub.domain/path/my%20dir/my%20file.txt?this=that&this=other&color=blue&v=5&name=John+Doe#somefragment";
        let trailing_slash = "safe://my.domain/";
        let double_q = "safe://my.domain/??foo=bar";

        let nrs = SafeUrl::from_url(nrsurl)?;
        let xor = SafeUrl::from_url(&nrs.to_xorurl_string())?;

        assert!(nrs.validate().is_ok());
        assert!(xor.validate().is_ok());

        assert!(SafeUrl::from_url(trailing_slash)?.validate().is_ok());
        assert!(SafeUrl::from_url(double_q)?.validate().is_ok());

        Ok(())
    }
}

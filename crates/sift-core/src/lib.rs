//! Core of Sift: reads camera files without decoding sensor data.

pub mod af;
#[cfg(target_vendor = "apple")]
pub mod apple;
pub mod container;
#[cfg(feature = "develop")]
pub mod develop;
pub mod exif;
pub mod export;
#[cfg(feature = "face")]
pub mod animal;
#[cfg(feature = "face")]
pub mod face;
pub mod formats;
pub mod histogram;
pub mod frame;
pub mod io;
pub mod jpeg;
pub mod live;
pub mod look;
pub mod nikon;
pub mod meta;
pub mod preview;
pub mod rating;
pub mod render;
pub mod stack;
pub mod tiff;
pub mod xmp;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("file structure is damaged or not recognised")]
    Malformed,
    #[error("no embedded preview in this file")]
    NoPreview,
    #[error("{0}")]
    Unsupported(&'static str),
    #[error("image decode failed: {0}")]
    Decode(String),
    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<image::ImageError> for Error {
    fn from(e: image::ImageError) -> Self {
        Error::Decode(e.to_string())
    }
}

impl From<jpeg_decoder::Error> for Error {
    fn from(e: jpeg_decoder::Error) -> Self {
        Error::Decode(e.to_string())
    }
}

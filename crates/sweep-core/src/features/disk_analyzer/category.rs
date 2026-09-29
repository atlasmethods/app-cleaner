//! File categories used by the disk analyzer, decided by extension only.
//!
//! The table below is the single source of truth (matching is ASCII case-insensitive on
//! the part after the last dot; a leading-dot name such as `.jpg` has no extension):
//!
//! | category     | extensions |
//! |--------------|------------|
//! | `pictures`   | jpg jpeg jpe jfif png gif bmp tif tiff webp heic heif avif svg ico psd xcf raw cr2 cr3 nef arw dng orf rw2 |
//! | `music`      | mp3 flac wav aac m4a ogg oga opus wma aiff aif mid midi ape alac amr |
//! | `documents`  | doc docx dot dotx odt rtf txt md pdf xls xlsx ods csv ppt pptx odp epub mobi tex pages numbers key djvu xps |
//! | `video`      | mp4 m4v mkv avi mov wmv flv webm mpg mpeg 3gp ogv m2ts vob |
//! | `compressed` | zip rar 7z tar gz tgz bz2 tbz2 xz txz zst lz lzma cab iso dmg |
//! | `email`      | eml msg pst ost mbox emlx olm |
//! | `other`      | everything else (including files without an extension) |
//!
//! `ts` is deliberately not a video extension (TypeScript is far more common), and `img`
//! is not an archive (too generic).

use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Pictures,
    Music,
    Documents,
    Video,
    Compressed,
    Email,
    Other,
}

impl Category {
    pub const ALL: [Category; 7] = [
        Category::Pictures,
        Category::Music,
        Category::Documents,
        Category::Video,
        Category::Compressed,
        Category::Email,
        Category::Other,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn key(self) -> &'static str {
        match self {
            Category::Pictures => "pictures",
            Category::Music => "music",
            Category::Documents => "documents",
            Category::Video => "video",
            Category::Compressed => "compressed",
            Category::Email => "email",
            Category::Other => "other",
        }
    }
}

const PICTURES: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "gif", "bmp", "tif", "tiff", "webp", "heic", "heif",
    "avif", "svg", "ico", "psd", "xcf", "raw", "cr2", "cr3", "nef", "arw", "dng", "orf", "rw2",
];
const MUSIC: &[&str] = &[
    "mp3", "flac", "wav", "aac", "m4a", "ogg", "oga", "opus", "wma", "aiff", "aif", "mid", "midi",
    "ape", "alac", "amr",
];
const DOCUMENTS: &[&str] = &[
    "doc", "docx", "dot", "dotx", "odt", "rtf", "txt", "md", "pdf", "xls", "xlsx", "ods", "csv",
    "ppt", "pptx", "odp", "epub", "mobi", "tex", "pages", "numbers", "key", "djvu", "xps",
];
const VIDEO: &[&str] = &[
    "mp4", "m4v", "mkv", "avi", "mov", "wmv", "flv", "webm", "mpg", "mpeg", "3gp", "ogv", "m2ts",
    "vob",
];
const COMPRESSED: &[&str] = &[
    "zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "tbz2", "xz", "txz", "zst", "lz", "lzma", "cab",
    "iso", "dmg",
];
const EMAIL: &[&str] = &["eml", "msg", "pst", "ost", "mbox", "emlx", "olm"];

/// Category of a file name / path.
pub fn classify(name: &OsStr) -> Category {
    let Some(ext) = Path::new(name).extension().and_then(OsStr::to_str) else {
        return Category::Other;
    };
    let ext = ext.to_ascii_lowercase();
    let ext = ext.as_str();
    for (list, cat) in [
        (PICTURES, Category::Pictures),
        (MUSIC, Category::Music),
        (DOCUMENTS, Category::Documents),
        (VIDEO, Category::Video),
        (COMPRESSED, Category::Compressed),
        (EMAIL, Category::Email),
    ] {
        if list.contains(&ext) {
            return cat;
        }
    }
    Category::Other
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(n: &str) -> Category {
        classify(OsStr::new(n))
    }

    #[test]
    fn table_examples() {
        assert_eq!(c("a.jpg"), Category::Pictures);
        assert_eq!(c("A.JPEG"), Category::Pictures);
        assert_eq!(c("song.Mp3"), Category::Music);
        assert_eq!(c("report.PDF"), Category::Documents);
        assert_eq!(c("movie.mkv"), Category::Video);
        assert_eq!(c("backup.tar.gz"), Category::Compressed);
        assert_eq!(c("x.7z"), Category::Compressed);
        assert_eq!(c("inbox.pst"), Category::Email);
        assert_eq!(c("mail.eml"), Category::Email);
        assert_eq!(c("main.rs"), Category::Other);
        assert_eq!(c("video.ts"), Category::Other);
        assert_eq!(c("noext"), Category::Other);
        assert_eq!(c(".jpg"), Category::Other, "a dotfile has no extension");
        assert_eq!(c("weird."), Category::Other);
    }

    #[test]
    fn no_extension_is_listed_twice() {
        let mut seen = std::collections::HashSet::new();
        for l in [PICTURES, MUSIC, DOCUMENTS, VIDEO, COMPRESSED, EMAIL] {
            for e in l {
                assert!(seen.insert(*e), "{e} appears in two categories");
                assert_eq!(*e, e.to_ascii_lowercase());
            }
        }
    }

    #[test]
    fn index_matches_all_order() {
        for (i, cat) in Category::ALL.iter().enumerate() {
            assert_eq!(cat.index(), i);
        }
    }
}

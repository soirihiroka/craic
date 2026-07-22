use crate::{
    ContentKind, FileProbe, FileSupportPatch, LanguageId, ResolvedFileSupport, RolePatch, registry,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum MatchLevel {
    Default,
    Extension,
    Pattern,
    ExactName,
    Content,
    Directory,
}

#[derive(Clone, Copy, Debug)]
pub struct FileSupportMatch {
    pub level: MatchLevel,
    pub patch: FileSupportPatch,
}

pub trait FileSupportResolver: Sync {
    fn resolve(&self, probe: &NormalizedFileProbe<'_>) -> Option<FileSupportMatch>;
}

pub struct NormalizedFileProbe<'a> {
    pub original: FileProbe<'a>,
    pub basename: String,
    pub extension: String,
}

pub struct DefaultResolver;

impl FileSupportResolver for DefaultResolver {
    fn resolve(&self, _probe: &NormalizedFileProbe<'_>) -> Option<FileSupportMatch> {
        Some(FileSupportMatch {
            level: MatchLevel::Default,
            patch: FileSupportPatch {
                language: Some(LanguageId::PlainText),
                content_kind: Some(ContentKind::Text),
                mime: Some("text/plain"),
                icon_name: Some("text-x-generic-symbolic"),
                display_name: Some("Text"),
                role: RolePatch::Replace(None),
            },
        })
    }
}

pub struct ExtensionResolver {
    pub extensions: &'static [&'static str],
    pub patch: FileSupportPatch,
}

impl FileSupportResolver for ExtensionResolver {
    fn resolve(&self, probe: &NormalizedFileProbe<'_>) -> Option<FileSupportMatch> {
        self.extensions
            .contains(&probe.extension.as_str())
            .then_some(FileSupportMatch {
                level: MatchLevel::Extension,
                patch: self.patch,
            })
    }
}

pub struct ExactNameResolver {
    pub names: &'static [&'static str],
    pub patch: FileSupportPatch,
}

impl FileSupportResolver for ExactNameResolver {
    fn resolve(&self, probe: &NormalizedFileProbe<'_>) -> Option<FileSupportMatch> {
        self.names
            .contains(&probe.basename.as_str())
            .then_some(FileSupportMatch {
                level: MatchLevel::ExactName,
                patch: self.patch,
            })
    }
}

pub struct NamePrefixResolver {
    pub prefixes: &'static [&'static str],
    pub patch: FileSupportPatch,
}

impl FileSupportResolver for NamePrefixResolver {
    fn resolve(&self, probe: &NormalizedFileProbe<'_>) -> Option<FileSupportMatch> {
        self.prefixes
            .iter()
            .any(|prefix| probe.basename.starts_with(prefix))
            .then_some(FileSupportMatch {
                level: MatchLevel::Pattern,
                patch: self.patch,
            })
    }
}

pub struct NameSuffixResolver {
    pub suffixes: &'static [&'static str],
    pub patch: FileSupportPatch,
}

impl FileSupportResolver for NameSuffixResolver {
    fn resolve(&self, probe: &NormalizedFileProbe<'_>) -> Option<FileSupportMatch> {
        self.suffixes
            .iter()
            .any(|suffix| probe.basename.ends_with(suffix))
            .then_some(FileSupportMatch {
                level: MatchLevel::Pattern,
                patch: self.patch,
            })
    }
}

pub struct ContainsWithExtensionResolver {
    pub fragment: &'static str,
    pub extensions: &'static [&'static str],
    pub patch: FileSupportPatch,
}

impl FileSupportResolver for ContainsWithExtensionResolver {
    fn resolve(&self, probe: &NormalizedFileProbe<'_>) -> Option<FileSupportMatch> {
        (probe.basename.contains(self.fragment)
            && self.extensions.contains(&probe.extension.as_str()))
        .then_some(FileSupportMatch {
            level: MatchLevel::Pattern,
            patch: self.patch,
        })
    }
}

pub struct MagicPrefixResolver {
    pub prefix: &'static [u8],
    pub patch: FileSupportPatch,
}

impl FileSupportResolver for MagicPrefixResolver {
    fn resolve(&self, probe: &NormalizedFileProbe<'_>) -> Option<FileSupportMatch> {
        probe
            .original
            .leading_bytes
            .is_some_and(|bytes| bytes.starts_with(self.prefix))
            .then_some(FileSupportMatch {
                level: MatchLevel::Content,
                patch: self.patch,
            })
    }
}

struct DirectoryResolver;

impl FileSupportResolver for DirectoryResolver {
    fn resolve(&self, probe: &NormalizedFileProbe<'_>) -> Option<FileSupportMatch> {
        probe.original.is_dir.then_some(FileSupportMatch {
            level: MatchLevel::Directory,
            patch: FileSupportPatch {
                language: Some(LanguageId::PlainText),
                content_kind: Some(ContentKind::Folder),
                mime: Some("inode/directory"),
                icon_name: Some("folder-symbolic"),
                display_name: Some("Folder"),
                role: RolePatch::Replace(None),
            },
        })
    }
}

static DEFAULT: DefaultResolver = DefaultResolver;
static DIRECTORY: DirectoryResolver = DirectoryResolver;

pub fn resolve(probe: FileProbe<'_>) -> ResolvedFileSupport {
    let basename = probe
        .path
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or(probe.path)
        .to_ascii_lowercase();
    let extension = basename
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .unwrap_or_default()
        .to_string();
    let normalized = NormalizedFileProbe {
        original: probe,
        basename,
        extension,
    };
    let mut matches = std::iter::once(&DEFAULT as &dyn FileSupportResolver)
        .chain(registry::resolvers())
        .chain(std::iter::once(&DIRECTORY as &dyn FileSupportResolver))
        .enumerate()
        .filter_map(|(order, resolver)| resolver.resolve(&normalized).map(|result| (order, result)))
        .collect::<Vec<_>>();
    matches.sort_by_key(|(order, result)| (result.level, *order));

    let (_, default_match) = matches.remove(0);
    let mut support = complete(default_match.patch);
    for (_, result) in matches {
        apply(&mut support, result.patch);
    }
    support
}

fn complete(patch: FileSupportPatch) -> ResolvedFileSupport {
    let RolePatch::Replace(role) = patch.role else {
        unreachable!("default file resolver must provide a role replacement");
    };
    ResolvedFileSupport {
        language: patch
            .language
            .expect("default file resolver must provide a language"),
        content_kind: patch
            .content_kind
            .expect("default file resolver must provide a content kind"),
        mime: patch
            .mime
            .expect("default file resolver must provide a MIME"),
        icon_name: patch
            .icon_name
            .expect("default file resolver must provide an icon"),
        display_name: patch
            .display_name
            .expect("default file resolver must provide a display name"),
        role,
    }
}

fn apply(support: &mut ResolvedFileSupport, patch: FileSupportPatch) {
    if let Some(language) = patch.language {
        support.language = language;
    }
    if let Some(content_kind) = patch.content_kind {
        support.content_kind = content_kind;
    }
    if let Some(mime) = patch.mime {
        support.mime = mime;
    }
    if let Some(icon_name) = patch.icon_name {
        support.icon_name = icon_name;
    }
    if let Some(display_name) = patch.display_name {
        support.display_name = display_name;
    }
    if let RolePatch::Replace(role) = patch.role {
        support.role = role;
    }
}

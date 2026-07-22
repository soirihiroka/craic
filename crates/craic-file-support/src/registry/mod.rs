pub(crate) mod documents;
pub(crate) mod media;
pub(crate) mod source;
pub(crate) mod special;

use crate::FileSupportResolver;

pub(crate) fn resolvers() -> impl Iterator<Item = &'static dyn FileSupportResolver> {
    source::RESOLVERS
        .iter()
        .chain(documents::RESOLVERS)
        .chain(media::RESOLVERS)
        .chain(special::RESOLVERS)
        .copied()
}

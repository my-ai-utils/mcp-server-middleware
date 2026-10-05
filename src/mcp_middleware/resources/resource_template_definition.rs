use crate::mcp_middleware::ResourceIcon;

/// Metadata of a resource template - a family of resources whose URIs share
/// one shape and are served by one handler, e.g.
/// `docs://rust-extensions/{topic}`. Listed in `resources/templates/list`.
pub trait ResourceTemplateDefinition {
    /// RFC 6570 level-1 template: literal text plus simple `{name}`
    /// variables. A variable stands for one non-empty piece of a path
    /// segment. Operators (`{+x}`, `{?x}`, ...), modifiers (`{x*}`,
    /// `{x:3}`) and two variables with no literal text between them are
    /// refused when the template is registered.
    const URI_TEMPLATE: &'static str;
    const TEMPLATE_NAME: &'static str;
    const DESCRIPTION: &'static str;
    /// The MIME type of every resource the template produces.
    const MIME_TYPE: &'static str;

    /// Optional human-readable title for display purposes
    fn get_title(&self) -> Option<&str> {
        None
    }

    /// Optional icons for display in user interfaces
    fn get_icons(&self) -> Vec<ResourceIcon> {
        Vec::new()
    }
}

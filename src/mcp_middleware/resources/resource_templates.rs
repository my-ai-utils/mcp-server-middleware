use std::collections::HashMap;
use std::sync::Arc;

use super::{McpResourceTemplateService, ResourceIcon, UriTemplate};

/// A registered resource template: its metadata, the parsed template and
/// the handler behind it.
pub struct ResourceTemplateExecutor {
    pub uri_template: &'static str,
    pub template_name: &'static str,
    pub description: &'static str,
    pub mime_type: &'static str,
    pub title: Option<String>,
    pub icons: Vec<ResourceIcon>,
    pub(crate) template: UriTemplate,
    pub holder: Arc<dyn McpResourceTemplateService + Send + Sync + 'static>,
}

/// Registry of resource templates. Filled while the middleware is being
/// built and read-only afterwards, so it needs no lock.
pub struct McpResourceTemplates {
    items: Vec<ResourceTemplateExecutor>,
}

impl McpResourceTemplates {
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// Registering the same `uri_template` twice replaces the previous
    /// entry, the way [`super::McpResources::add`] does for a URI.
    pub fn add(&mut self, executor: ResourceTemplateExecutor) {
        match self
            .items
            .iter_mut()
            .find(|item| item.uri_template == executor.uri_template)
        {
            Some(existing) => *existing = executor,
            None => self.items.push(executor),
        }
    }

    /// Every template, in the order of registration.
    pub fn get_list(&self) -> &[ResourceTemplateExecutor] {
        self.items.as_slice()
    }

    pub fn has_templates(&self) -> bool {
        !self.items.is_empty()
    }

    /// The template `uri` belongs to, with its variables. When several
    /// templates match, the one with the most literal text - the most
    /// specific one - wins; on a tie, the one registered first.
    pub fn find(&self, uri: &str) -> Option<(&ResourceTemplateExecutor, HashMap<String, String>)> {
        let mut found: Option<(&ResourceTemplateExecutor, HashMap<String, String>)> = None;

        for item in self.items.iter() {
            let Some(variables) = item.template.match_uri(uri) else {
                continue;
            };

            let more_specific = match &found {
                Some((current, _)) => item.template.literal_len() > current.template.literal_len(),
                None => true,
            };

            if more_specific {
                found = Some((item, variables));
            }
        }

        found
    }
}

impl Default for McpResourceTemplates {
    fn default() -> Self {
        Self::new()
    }
}

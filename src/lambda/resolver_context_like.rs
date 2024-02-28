use std::collections::HashMap;

use async_graphql::context::SelectionField;
use async_graphql::dynamic::ResolverContext;
use async_graphql::{Name, ServerError, Value};
use indexmap::IndexMap;

pub struct ResolverContextWithArgs<'a> {
    source: Box<&'a dyn ResolverContextLike<'a>>,
    args: &'a HashMap<String, serde_json::Value>,
}

impl<'a> ResolverContextWithArgs<'a> {
    pub fn new(
        source: &'a dyn ResolverContextLike<'a>,
        args: &'a HashMap<String, serde_json::Value>,
    ) -> Self {
        ResolverContextWithArgs { source: Box::new(source), args }
    }
}

impl<'a> ResolverContextLike<'a> for ResolverContextWithArgs<'a> {
    fn value(&'a self) -> Option<&Value> {
        self.source.value()
    }

    fn args(&'a self) -> Option<IndexMap<Name, Value>> {
        let mut args = self
            .args
            .iter()
            .map(|(k, v)| (Name::new(k), Value::from(v.to_string())))
            .collect::<IndexMap<Name, Value>>();

        self.source.args().map(|_args| args.extend(_args.clone()));

        Some(args)
    }

    fn with_args(
        &'a self,
        args: &'a HashMap<String, serde_json::Value>,
    ) -> ResolverContextWithArgs<'a> {
        ResolverContextWithArgs::new(self, args)
    }

    fn field(&'a self) -> Option<SelectionField> {
        self.source.field()
    }

    fn add_error(&'a self, error: ServerError) {
        self.source.add_error(error)
    }
}

pub trait ResolverContextLike<'a> {
    fn value(&'a self) -> Option<&'a Value>;
    fn args(&'a self) -> Option<IndexMap<Name, Value>>;
    fn with_args(
        &'a self,
        args: &'a HashMap<String, serde_json::Value>,
    ) -> ResolverContextWithArgs<'a>;
    fn field(&'a self) -> Option<SelectionField>;
    fn add_error(&'a self, error: ServerError);
}

pub struct EmptyResolverContext;

impl<'a> ResolverContextLike<'a> for EmptyResolverContext {
    fn value(&'a self) -> Option<&'a Value> {
        None
    }

    fn args(&'a self) -> Option<IndexMap<Name, Value>> {
        None
    }

    fn with_args(
        &'a self,
        args: &'a HashMap<String, serde_json::Value>,
    ) -> ResolverContextWithArgs<'a> {
        ResolverContextWithArgs::new(self, args)
    }

    fn field(&'a self) -> Option<SelectionField> {
        None
    }

    fn add_error(&'a self, _: ServerError) {}
}

impl<'a> ResolverContextLike<'a> for ResolverContext<'a> {
    fn value(&'a self) -> Option<&'a Value> {
        self.parent_value.as_value()
    }

    fn args(&'a self) -> Option<IndexMap<Name, Value>> {
        Some(self.args.as_index_map().clone())
    }

    fn with_args(
        &'a self,
        args: &'a HashMap<String, serde_json::Value>,
    ) -> ResolverContextWithArgs<'a> {
        ResolverContextWithArgs::new(self, args)
    }

    fn field(&'a self) -> Option<SelectionField> {
        Some(self.ctx.field())
    }

    fn add_error(&'a self, error: ServerError) {
        self.ctx.add_error(error)
    }
}

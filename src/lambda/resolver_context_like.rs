use std::collections::HashMap;

use async_graphql::context::SelectionField;
use async_graphql::dynamic::ResolverContext;
use async_graphql::{Name, ServerError, Value};
use indexmap::IndexMap;

pub struct ResolverContextWithArgs<'a> {
    source: Box<&'a dyn ResolverContextLike<'a>>,
    args: &'a HashMap<String, serde_json::Value>,
}

impl<'a> ResolverContextLike<'a> for ResolverContextWithArgs<'a> {
    fn value(&'a self) -> Option<&Value> {
        self.source.value()
    }

    fn args(&'a self) -> Option<IndexMap<Name, Value>> {
        let mut args = self.source.args().unwrap_or_default();

        args.extend(
            self.args
                .iter()
                .map(|(k, v)| (Name::new(k), Value::from(v.to_string())))
                .collect::<IndexMap<Name, Value>>(),
        );

        Some(args)
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
    ) -> ResolverContextWithArgs<'a>
    where
        Self: Sized,
    {
        ResolverContextWithArgs { source: Box::new(self), args }
    }
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

    fn field(&'a self) -> Option<SelectionField> {
        Some(self.ctx.field())
    }

    fn add_error(&'a self, error: ServerError) {
        self.ctx.add_error(error)
    }
}

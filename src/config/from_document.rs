#![allow(clippy::too_many_arguments)]

use std::collections::BTreeMap;

use async_graphql::parser::types::{
  BaseType, ConstDirective, EnumType, FieldDefinition, InputObjectType, InputValueDefinition, SchemaDefinition,
  ServiceDocument, Type, TypeDefinition, TypeKind, TypeSystemDefinition, UnionType,
};
use async_graphql::parser::Positioned;
use async_graphql::Name;

use super::introspection::IntrospectionResult;
use crate::config::group_by::GroupBy;
use crate::config::introspection::introspect_endpoint;
use crate::config::{self, Config, GraphQL, GraphQLSource, Http, RootSchema, Server, Union, Upstream};
use crate::directive::DirectiveCodec;
use crate::valid::NeoValid;

pub async fn from_document(
  doc: ServiceDocument,
  initialize_introspection_cache: Option<fn() -> BTreeMap<String, IntrospectionResult>>,
) -> NeoValid<Config, String> {
  let config = schema_definition(&doc)
    .and_then(|sd| server(sd).zip(upstream(sd)).zip(graphql(&doc, sd)))
    .map(|((server, upstream), graphql)| Config { server, upstream, graphql, introspection_cache: BTreeMap::new() });

  match config {
    NeoValid(Ok(mut config)) => {
      if let Some(initialize) = initialize_introspection_cache {
        config.introspection_cache = initialize()
      }
      update_introspection_results(config).await
    }
    NeoValid(Err(e)) => NeoValid(Err(e)),
  }
}

fn graphql(doc: &ServiceDocument, sd: &SchemaDefinition) -> NeoValid<GraphQL, String> {
  let type_definitions: Vec<_> = doc
    .definitions
    .iter()
    .filter_map(|def| match def {
      TypeSystemDefinition::Type(type_definition) => Some(type_definition),
      _ => None,
    })
    .collect();

  to_types(&type_definitions)
    .map(|types| (GraphQL { schema: to_root_schema(sd), types, unions: to_union_types(&type_definitions) }))
}

fn schema_definition(doc: &ServiceDocument) -> NeoValid<&SchemaDefinition, String> {
  let p = doc.definitions.iter().find_map(|def| match def {
    TypeSystemDefinition::Schema(schema_definition) => Some(&schema_definition.node),
    _ => None,
  });
  p.map_or_else(
    || NeoValid::fail("schema not found".to_string()).trace("schema"),
    NeoValid::succeed,
  )
}

fn process_schema_directives<'a, T: DirectiveCodec<'a, T> + Default>(
  schema_definition: &'a SchemaDefinition,
  directive_name: &str,
) -> NeoValid<T, String> {
  let mut res = NeoValid::succeed(T::default());
  for directive in schema_definition.directives.iter() {
    if directive.node.name.node.as_ref() == directive_name {
      res = T::from_directive(&directive.node);
    }
  }
  res
}

fn server(schema_definition: &SchemaDefinition) -> NeoValid<Server, String> {
  process_schema_directives(schema_definition, "server")
}
fn upstream(schema_definition: &SchemaDefinition) -> NeoValid<Upstream, String> {
  process_schema_directives(schema_definition, "upstream")
}
fn to_root_schema(schema_definition: &SchemaDefinition) -> RootSchema {
  let query = schema_definition.query.as_ref().map(pos_name_to_string);
  let mutation = schema_definition.mutation.as_ref().map(pos_name_to_string);
  let subscription = schema_definition.subscription.as_ref().map(pos_name_to_string);

  RootSchema { query, mutation, subscription }
}
fn pos_name_to_string(pos: &Positioned<Name>) -> String {
  pos.node.to_string()
}
fn to_types(type_definitions: &Vec<&Positioned<TypeDefinition>>) -> NeoValid<BTreeMap<String, config::Type>, String> {
  NeoValid::from_iter(type_definitions, |type_definition| {
    let type_name = pos_name_to_string(&type_definition.node.name);
    match type_definition.node.kind.clone() {
      TypeKind::Object(object_type) => to_object_type(
        &object_type.fields,
        &type_definition.node.description,
        false,
        &object_type.implements,
      )
      .some(),
      TypeKind::Interface(interface_type) => to_object_type(
        &interface_type.fields,
        &type_definition.node.description,
        true,
        &interface_type.implements,
      )
      .some(),
      TypeKind::Enum(enum_type) => NeoValid::succeed(Some(to_enum(enum_type))),
      TypeKind::InputObject(input_object_type) => to_input_object(input_object_type).some(),
      TypeKind::Union(_) => NeoValid::none(),
      TypeKind::Scalar => NeoValid::succeed(Some(to_scalar_type())),
    }
    .map(|option| (type_name, option))
  })
  .map(|vec| {
    BTreeMap::from_iter(
      vec
        .into_iter()
        .filter_map(|(name, option)| option.map(|tpe| (name, tpe))),
    )
  })
}
fn to_scalar_type() -> config::Type {
  config::Type { scalar: true, ..Default::default() }
}
fn to_union_types(type_definitions: &Vec<&Positioned<TypeDefinition>>) -> BTreeMap<String, Union> {
  let mut unions = BTreeMap::new();
  for type_definition in type_definitions {
    let type_name = pos_name_to_string(&type_definition.node.name);
    let type_opt = match type_definition.node.kind.clone() {
      TypeKind::Union(union_type) => to_union(
        union_type,
        &type_definition.node.description.as_ref().map(|pos| pos.node.clone()),
      ),
      _ => continue,
    };
    unions.insert(type_name, type_opt);
  }
  unions
}
fn to_object_type(
  fields: &Vec<Positioned<FieldDefinition>>,
  description: &Option<Positioned<String>>,
  interface: bool,
  implements: &[Positioned<Name>],
) -> NeoValid<config::Type, String> {
  to_fields(fields).map(|fields| {
    let doc = description.as_ref().map(|pos| pos.node.clone());
    let implements = implements.iter().map(|pos| pos.node.to_string()).collect();
    config::Type { fields, doc, interface, implements, ..Default::default() }
  })
}
fn to_enum(enum_type: EnumType) -> config::Type {
  let variants = enum_type
    .values
    .iter()
    .map(|value| value.node.value.to_string())
    .collect();
  config::Type { variants: Some(variants), ..Default::default() }
}
fn to_input_object(input_object_type: InputObjectType) -> NeoValid<config::Type, String> {
  to_input_object_fields(&input_object_type.fields).map(|fields| config::Type { fields, ..Default::default() })
}

fn to_fields_inner<T, F>(fields: &Vec<Positioned<T>>, transform: F) -> NeoValid<BTreeMap<String, config::Field>, String>
where
  F: Fn(&T) -> NeoValid<config::Field, String>,
  T: HasName,
{
  NeoValid::from_iter(fields, |field| {
    let field_name = pos_name_to_string(field.node.name());
    transform(&field.node).map(|field| (field_name, field))
  })
  .map(BTreeMap::from_iter)
}
fn to_fields(fields: &Vec<Positioned<FieldDefinition>>) -> NeoValid<BTreeMap<String, config::Field>, String> {
  to_fields_inner(fields, to_field)
}
fn to_input_object_fields(
  input_object_fields: &Vec<Positioned<InputValueDefinition>>,
) -> NeoValid<BTreeMap<String, config::Field>, String> {
  to_fields_inner(input_object_fields, to_input_object_field)
}
fn to_field(field_definition: &FieldDefinition) -> NeoValid<config::Field, String> {
  to_common_field(
    &field_definition.ty.node,
    &field_definition.ty.node.base,
    field_definition.ty.node.nullable,
    to_args(field_definition),
    &field_definition.description,
    &field_definition.directives,
  )
}
fn to_input_object_field(field_definition: &InputValueDefinition) -> NeoValid<config::Field, String> {
  to_common_field(
    &field_definition.ty.node,
    &field_definition.ty.node.base,
    field_definition.ty.node.nullable,
    BTreeMap::new(),
    &field_definition.description,
    &field_definition.directives,
  )
}
fn to_common_field(
  type_: &Type,
  base: &BaseType,
  nullable: bool,
  args: BTreeMap<String, config::Arg>,
  description: &Option<Positioned<String>>,
  directives: &[Positioned<ConstDirective>],
) -> NeoValid<config::Field, String> {
  let type_of = to_type_of(type_);
  let list = matches!(&base, BaseType::List(_));
  let list_type_required = matches!(&base, BaseType::List(ty) if !ty.nullable);
  let doc = description.as_ref().map(|pos| pos.node.clone());
  let modify = to_modify(directives);
  let inline = to_inline(directives);
  to_http(directives)
    .zip(to_graphqlsource(directives))
    .map(|(http, graphql_source)| {
      let unsafe_operation = to_unsafe_operation(directives);
      let group_by = to_batch(directives);
      let const_field = to_const_field(directives);
      config::Field {
        type_of,
        list,
        required: !nullable,
        list_type_required,
        args,
        doc,
        modify,
        inline,
        http,
        unsafe_operation,
        group_by,
        const_field,
        graphql_source,
      }
    })
}
fn to_unsafe_operation(directives: &[Positioned<ConstDirective>]) -> Option<config::Unsafe> {
  directives.iter().find_map(|directive| {
    if directive.node.name.node == "unsafe" {
      config::Unsafe::from_directive(&directive.node).to_result().ok()
    } else {
      None
    }
  })
}
fn to_type_of(type_: &Type) -> String {
  match &type_.base {
    BaseType::Named(name) => name.to_string(),
    BaseType::List(ty) => match &ty.base {
      BaseType::Named(name) => name.to_string(),
      _ => "".to_string(),
    },
  }
}
fn to_args(field_definition: &FieldDefinition) -> BTreeMap<String, config::Arg> {
  let mut args: BTreeMap<String, config::Arg> = BTreeMap::new();

  for arg in field_definition.arguments.iter() {
    let arg_name = pos_name_to_string(&arg.node.name);
    let arg_val = to_arg(&arg.node);
    args.insert(arg_name, arg_val);
  }

  args
}
fn to_arg(input_value_definition: &InputValueDefinition) -> config::Arg {
  let type_of = to_type_of(&input_value_definition.ty.node);
  let list = matches!(&input_value_definition.ty.node.base, BaseType::List(_));
  let required = !input_value_definition.ty.node.nullable;
  let doc = input_value_definition.description.as_ref().map(|pos| pos.node.clone());
  let modify = to_modify(&input_value_definition.directives);
  let default_value = if let Some(pos) = input_value_definition.default_value.as_ref() {
    let value = &pos.node;
    serde_json::to_value(value).ok()
  } else {
    None
  };
  config::Arg { type_of, list, required, doc, modify, default_value }
}
fn to_modify(directives: &[Positioned<ConstDirective>]) -> Option<config::ModifyField> {
  directives.iter().find_map(|directive| {
    if directive.node.name.node == "modify" {
      config::ModifyField::from_directive(&directive.node).to_result().ok()
    } else {
      None
    }
  })
}
fn to_inline(directives: &[Positioned<ConstDirective>]) -> Option<config::InlineType> {
  directives.iter().find_map(|directive| {
    if directive.node.name.node == "inline" {
      config::InlineType::from_directive(&directive.node).to_result().ok()
    } else {
      None
    }
  })
}
fn to_http(directives: &[Positioned<ConstDirective>]) -> NeoValid<Option<config::Http>, String> {
  for directive in directives {
    if directive.node.name.node == "http" {
      return Http::from_directive(&directive.node).map(Some);
    }
  }
  NeoValid::succeed(None)
}
fn to_union(union_type: UnionType, doc: &Option<String>) -> Union {
  let types = union_type
    .members
    .iter()
    .map(|member| member.node.to_string())
    .collect();
  Union { types, doc: doc.clone() }
}
fn to_batch(directives: &[Positioned<ConstDirective>]) -> Option<GroupBy> {
  directives.iter().find_map(|directive| {
    if directive.node.name.node == "groupBy" {
      GroupBy::from_directive(&directive.node).to_result().ok()
    } else {
      None
    }
  })
}
fn to_const_field(directives: &[Positioned<ConstDirective>]) -> Option<config::ConstField> {
  directives.iter().find_map(|directive| {
    if directive.node.name.node == "const" {
      config::ConstField::from_directive(&directive.node).to_result().ok()
    } else {
      None
    }
  })
}
fn to_graphqlsource(directives: &[Positioned<ConstDirective>]) -> NeoValid<Option<config::GraphQLSource>, String> {
  for directive in directives {
    if directive.node.name.node == "graphql" {
      return GraphQLSource::from_directive(&directive.node).map(Some);
    }
  }
  NeoValid::succeed(None)
}
async fn update_introspection_results(mut config: Config) -> NeoValid<Config, String> {
  for type_ in config.graphql.types.values_mut() {
    for field in type_.fields.values_mut() {
      match &field.graphql_source {
        Some(graphql_source) => {
          let updated = update_introspection(graphql_source, &mut config.introspection_cache).await;
          match &updated {
            NeoValid(Ok(source)) => {
              field.graphql_source = Some(source.clone());
            }
            NeoValid(Err(e)) => {
              return NeoValid(Err(e.clone()));
            }
          }
        }
        None => {}
      }
    }
  }
  NeoValid::succeed(config)
}
async fn update_introspection(
  graphqlsource: &config::GraphQLSource,
  introspection_cache: &mut BTreeMap<String, IntrospectionResult>,
) -> NeoValid<config::GraphQLSource, String> {
  let mut updated: GraphQLSource = graphqlsource.clone();
  match &graphqlsource.base_url {
    Some(base_url) => {
      let introspection_result = introspection_cache.get(base_url);
      match introspection_result {
        Some(introspection) => {
          updated.introspection = Some(introspection.clone());
          NeoValid::succeed(updated)
        }
        None => {
          let introspection_result = introspect_endpoint(base_url).await;
          match introspection_result {
            Ok(introspection) => {
              updated.introspection = Some(introspection.clone());
              introspection_cache.insert(base_url.clone(), introspection.clone());
              NeoValid::succeed(updated)
            }
            Err(e) => NeoValid::fail(e.to_string()),
          }
        }
      }
    }
    None => NeoValid::fail("No base url found for graphql directive".to_string()).trace("introspection"),
  }
}
trait HasName {
  fn name(&self) -> &Positioned<Name>;
}
impl HasName for FieldDefinition {
  fn name(&self) -> &Positioned<Name> {
    &self.name
  }
}
impl HasName for InputValueDefinition {
  fn name(&self) -> &Positioned<Name> {
    &self.name
  }
}

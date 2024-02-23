use std::collections::hash_map::Iter;

use crate::blueprint::*;
use crate::config::group_by::GroupBy;
use crate::config::{Field, GraphQLOperationType};
use crate::lambda::{DataLoaderId, Expression, IO};
use crate::mustache::{Mustache, Segment};
use crate::try_fold::TryFold;
use crate::valid::{Valid, Validator};
use crate::{config, graphql, grpc, http};

fn find_value<'a>(args: &'a Iter<'a, String, String>, key: &'a String) -> Option<&'a String> {
    args.clone()
        .find_map(|(k, value)| if k == key { Some(value) } else { None })
}

pub fn update_call(
    operation_type: &GraphQLOperationType,
) -> TryFold<'_, (&ConfigModule, &Field, &config::Type, &str), FieldDefinition, String> {
    TryFold::<(&ConfigModule, &Field, &config::Type, &str), FieldDefinition, String>::new(
        move |(config, field, _, _), b_field| {
            let Some(call) = &field.call else {
                return Valid::succeed(b_field);
            };

            compile_call(field, config, call, operation_type)
                .map(|resolver| b_field.resolver(Some(resolver)))
        },
    )
}

struct Http {
    pub req_template: http::RequestTemplate,
    pub group_by: Option<GroupBy>,
    pub dl_id: Option<DataLoaderId>,
}

struct GraphQL {
    pub req_template: graphql::RequestTemplate,
    pub field_name: String,
    pub batch: bool,
    pub dl_id: Option<DataLoaderId>,
}

struct Grpc {
    pub req_template: grpc::RequestTemplate,
    pub group_by: Option<GroupBy>,
    pub dl_id: Option<DataLoaderId>,
}

impl TryFrom<Expression> for Http {
    type Error = String;

    fn try_from(expr: Expression) -> Result<Self, Self::Error> {
        match expr {
            Expression::IO(IO::Http { req_template, group_by, dl_id }) => {
                Ok(Http { req_template, group_by, dl_id })
            }
            _ => Err("not an http expression".to_string()),
        }
    }
}

impl TryFrom<Expression> for GraphQL {
    type Error = String;

    fn try_from(expr: Expression) -> Result<Self, Self::Error> {
        match expr {
            Expression::IO(IO::GraphQL { req_template, field_name, batch, dl_id }) => {
                Ok(GraphQL { req_template, field_name, batch, dl_id })
            }
            _ => Err("not a graphql expression".to_string()),
        }
    }
}

impl TryFrom<Expression> for Grpc {
    type Error = String;

    fn try_from(expr: Expression) -> Result<Self, Self::Error> {
        match expr {
            Expression::IO(IO::Grpc { req_template, group_by, dl_id }) => {
                Ok(Grpc { req_template, group_by, dl_id })
            }
            _ => Err("not a grpc expression".to_string()),
        }
    }
}

pub fn compile_call(
    field: &Field,
    config_module: &ConfigModule,
    call: &config::Call,
    operation_type: &GraphQLOperationType,
) -> Valid<Expression, String> {
    get_field_and_field_name(call, config_module).and_then(|(_field, field_name, args)| {
        let empties: Vec<(&String, &config::Arg)> = _field
            .args
            .iter()
            .filter(|(k, _)| !args.clone().any(|(k1, _)| k1.eq(*k)))
            .collect();

        if empties.len().gt(&0) {
            return Valid::fail(format!(
                "no argument {} found",
                empties
                    .iter()
                    .map(|(k, _)| format!("'{}'", k))
                    .collect::<Vec<String>>()
                    .join(", ")
            ))
            .trace(field_name.as_str());
        }

        if let Some(http) = _field.http.clone() {
            transform_http(config_module, field, http, &args)
        } else if let Some(graphql) = _field.graphql.clone() {
            transform_graphql(config_module, operation_type, graphql, &args)
        } else if let Some(grpc) = _field.grpc.clone() {
            transform_grpc(
                CompileGrpc {
                    config_module,
                    operation_type,
                    field,
                    grpc: &grpc,
                    validate_with_schema: false,
                },
                args,
            )
        } else {
            return Valid::fail(format!("{} field has no resolver", field_name));
        }
    })
}

fn transform_grpc(
    inputs: CompileGrpc<'_>,
    args: Iter<'_, String, String>,
) -> Valid<Expression, String> {
    compile_grpc(inputs).and_then(|expr| {
        let grpc = Grpc::try_from(expr).unwrap();

        Valid::succeed(
            grpc.req_template
                .clone()
                .url(replace_mustache_value(&grpc.req_template.url, &args)),
        )
        .map(|req_template| {
            req_template.clone().headers(
                req_template
                    .headers
                    .iter()
                    .map(replace_mustache(&args))
                    .collect(),
            )
        })
        .map(|req_template| {
            req_template.clone().body(
                req_template
                    .body
                    .map(|body| replace_mustache_value(&body, &args)),
            )
        })
        .map(|req_template| {
            Expression::IO(IO::Grpc { req_template, group_by: grpc.group_by, dl_id: grpc.dl_id })
        })
    })
}

fn transform_graphql(
    config_module: &ConfigModule,
    operation_type: &GraphQLOperationType,
    graphql: config::GraphQL,
    args: &Iter<'_, String, String>,
) -> Valid<Expression, String> {
    compile_graphql(config_module, operation_type, &graphql).and_then(|expr| {
        let graphql = GraphQL::try_from(expr).unwrap();

        Valid::succeed(
            graphql.req_template.clone().headers(
                graphql
                    .req_template
                    .headers
                    .iter()
                    .map(replace_mustache(args))
                    .collect(),
            ),
        )
        .map(|req_template| {
            if req_template.operation_arguments.is_some() {
                let operation_arguments = req_template
                    .clone()
                    .operation_arguments
                    .unwrap()
                    .iter()
                    .map(replace_mustache(args))
                    .collect();

                req_template.operation_arguments(Some(operation_arguments))
            } else {
                req_template
            }
        })
        .map(|req_template| {
            Expression::IO(IO::GraphQL {
                req_template,
                field_name: graphql.field_name,
                batch: graphql.batch,
                dl_id: graphql.dl_id,
            })
        })
    })
}

fn transform_http(
    config_module: &ConfigModule,
    field: &Field,
    http: config::Http,
    args: &Iter<'_, String, String>,
) -> Valid<Expression, String> {
    compile_http(config_module, field, &http).and_then(|expr| {
        let http = Http::try_from(expr).unwrap();

        Valid::succeed(
            http.req_template
                .clone()
                .root_url(replace_mustache_value(&http.req_template.root_url, args)),
        )
        .map(|req_template| {
            req_template.clone().query(
                req_template
                    .clone()
                    .query
                    .iter()
                    .map(replace_mustache(args))
                    .collect(),
            )
        })
        .map(|req_template| {
            req_template.clone().headers(
                req_template
                    .headers
                    .iter()
                    .map(replace_mustache(args))
                    .collect(),
            )
        })
        .map(|req_template| {
            req_template.clone().body_path(
                req_template
                    .body_path
                    .map(|body_path| replace_mustache_value(&body_path, args)),
            )
        })
        .map(|req_template| {
            Expression::IO(IO::Http { req_template, dl_id: http.dl_id, group_by: http.group_by })
        })
    })
}

fn get_type_and_field(call: &config::Call) -> Option<(String, String)> {
    if let Some(query) = &call.query {
        Some(("Query".to_string(), query.clone()))
    } else {
        call.mutation
            .as_ref()
            .map(|mutation| ("Mutation".to_string(), mutation.clone()))
    }
}

fn get_field_and_field_name<'a>(
    call: &'a config::Call,
    config_module: &'a ConfigModule,
) -> Valid<(&'a Field, String, Iter<'a, String, String>), String> {
    Valid::from_option(
        get_type_and_field(call),
        "call must have query or mutation".to_string(),
    )
    .and_then(|(type_name, field_name)| {
        Valid::from_option(
            config_module.config.find_type(&type_name),
            format!("{} type not found on config", type_name),
        )
        .and_then(|query_type| {
            Valid::from_option(
                query_type.fields.get(&field_name),
                format!("{} field not found", field_name),
            )
        })
        .fuse(Valid::succeed(field_name))
        .fuse(Valid::succeed(call.args.iter()))
        .into()
    })
}

fn replace_mustache_value(value: &Mustache, args: &Iter<'_, String, String>) -> Mustache {
    value
        .get_segments()
        .iter()
        .map(|segment| match segment {
            Segment::Literal(literal) => Segment::Literal(literal.clone()),
            Segment::Expression(expression) => {
                if expression[0] == "args" {
                    let value = find_value(args, &expression[1]).unwrap();
                    let item = Mustache::parse(value).unwrap();

                    let expression = item.get_segments().first().unwrap().to_owned().to_owned();

                    expression
                } else {
                    Segment::Expression(expression.clone())
                }
            }
        })
        .collect::<Vec<Segment>>()
        .into()
}

fn replace_mustache<'a, T: Clone>(
    args: &'a Iter<'a, String, String>,
) -> impl Fn(&(T, Mustache)) -> (T, Mustache) + 'a {
    move |(key, value)| {
        let value: Mustache = replace_mustache_value(value, args);

        (key.clone().to_owned(), value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_from_http_fail() {
        let expr = Expression::Literal(DynamicValue::Value("test".into()));

        let http = Http::try_from(expr);

        assert!(http.is_err());
    }

    #[test]
    fn try_from_graphql_fail() {
        let expr = Expression::Literal(DynamicValue::Value("test".into()));

        let graphql = GraphQL::try_from(expr);

        assert!(graphql.is_err());
    }

    #[test]
    fn try_from_grpc_fail() {
        let expr = Expression::Literal(DynamicValue::Value("test".into()));

        let grpc = Grpc::try_from(expr);

        assert!(grpc.is_err());
    }
}
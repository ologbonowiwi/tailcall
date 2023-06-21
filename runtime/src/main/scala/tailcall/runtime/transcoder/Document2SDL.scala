package tailcall.runtime.transcoder

import caliban.GraphQL
import caliban.InputValue.ListValue
import caliban.execution.Feature
import caliban.introspection.adt.__Directive
import caliban.parsing.adt.Definition.TypeSystemDefinition
import caliban.parsing.adt.Definition.TypeSystemDefinition.TypeDefinition
import caliban.parsing.adt.{Definition, Document}
import caliban.schema.{Operation, RootSchemaBuilder, Step}
import caliban.tools.RemoteSchema
import caliban.wrappers.Wrapper
import tailcall.runtime.internal.TValid

trait Document2SDL {
  final def toSDL(document: Document): TValid[Nothing, String] =
    TValid.succeed {
      val normalized        = normalize(document)
      val schema            = RemoteSchema.parseRemoteSchema(normalized)
      val extDirectiveTypes = document.objectTypeDefinitions.map(definition => {
        val extendsDirective = definition.directives.find(directive => directive.name == "extends")
        extendsDirective match {
          case Some(value) =>
            val typesOption = value.arguments.get("types")
            typesOption match {
              case Some(inputValue) => inputValue match {
                  case ListValue(values) => values.map(_.toString)
                  case _                 => Nil
                }
              case _                => Nil
            }
          case None        => Nil
        }
      }).flatten

      val additionalTypes = schema match {
        case Some(s) => s.types.filter(t => extDirectiveTypes.contains(s"\"${t.name.getOrElse("")}\""))
        case None    => Nil
      }

      new GraphQL[Any] {
        override protected val schemaBuilder: RootSchemaBuilder[Any]   = {
          RootSchemaBuilder(
            schema.map(_.queryType).map(__type => Operation(__type, Step.NullStep)),
            schema.flatMap(_.mutationType).map(__type => Operation(__type, Step.NullStep)),
            None,
            schemaDirectives = normalized.schemaDefinition.map(_.directives).getOrElse(Nil),
          )
        }
        override protected val wrappers: List[Wrapper[Any]]            = Nil
        override protected val additionalDirectives: List[__Directive] = Nil
        override protected val features: Set[Feature]                  = Set.empty
      }.withAdditionalTypes(additionalTypes).render
    }

  /**
   * Normalize the document by sorting definitions and
   * fields. This ensures consistent SDL renders for similar
   * documents.
   */
  final private def normalize(document: Document): Document = {
    document.copy(definitions = document.definitions.map {
      case definition: TypeDefinition.ObjectTypeDefinition => definition.copy(fields = definition.fields.sortBy(_.name))
      case definition: TypeDefinition.InputObjectTypeDefinition => definition
          .copy(fields = definition.fields.sortBy(_.name))
      case definition                                           => definition
    }.sortBy[String] {
      case _: Definition.ExecutableDefinition          => ""
      case _: Definition.TypeSystemExtension           => ""
      case definition: Definition.TypeSystemDefinition => definition match {
          case _: TypeSystemDefinition.DirectiveDefinition     => "a"
          case _: TypeSystemDefinition.SchemaDefinition        => "b"
          case definition: TypeSystemDefinition.TypeDefinition => definition match {
              case _: TypeDefinition.ScalarTypeDefinition      => "c" + definition.name
              case _: TypeDefinition.InputObjectTypeDefinition => "d" + definition.name
              case _: TypeDefinition.ObjectTypeDefinition      => "e" + definition.name
              case _: TypeDefinition.InterfaceTypeDefinition   => definition.name
              case _: TypeDefinition.EnumTypeDefinition        => definition.name
              case _: TypeDefinition.UnionTypeDefinition       => definition.name
            }
        }
    })
  }
}

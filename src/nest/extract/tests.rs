use std::path::Path;

use proptest::prelude::*;

use super::*;
use crate::route::BodyShape;

const FILE: &str = "src/users/users.controller.ts";

fn analyze(source: &str) -> FileAnalysis {
    extract_controllers(Path::new(FILE), source, None)
}

/// Analyze a source that is expected to be diagnostic-free.
fn clean(source: &str) -> Vec<Controller> {
    let analysis = analyze(source);
    assert!(
        analysis.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        analysis.diagnostics
    );
    analysis.controllers
}

/// `(METHOD, path)` of every route, in order.
fn routes(controllers: &[Controller]) -> Vec<(String, String)> {
    controllers
        .iter()
        .flat_map(|c| &c.routes)
        .map(|r| (r.http_method.to_string(), r.path.to_string()))
        .collect()
}

fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|(m, p)| ((*m).to_owned(), (*p).to_owned()))
        .collect()
}

fn single_route(decorators: &str) -> Route {
    let source = format!("@Controller('users')\nclass C {{\n  {decorators}\n  handler() {{}}\n}}");
    let mut controllers = clean(&source);
    controllers.remove(0).routes.remove(0)
}

// ------------------------------------------------------------ controllers

#[test]
fn controller_without_arguments_has_the_root_path() {
    let c = clean("@Controller() export class UsersController {}");
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].name, "UsersController");
    assert_eq!(c[0].path, RoutePath::Static("/".into()));
    assert!(c[0].routes.is_empty());
}

#[test]
fn controller_with_string_path() {
    let c = clean("@Controller('users')\nexport class UsersController {}");
    assert_eq!(c[0].path, RoutePath::Static("/users".into()));
}

#[test]
fn controller_with_parameterized_path() {
    let c = clean("@Controller('users/:id') class A {}");
    assert_eq!(c[0].path, RoutePath::Static("/users/:id".into()));
}

#[test]
fn controller_path_formats_are_equivalent() {
    let variants = [
        "@Controller('users') class A {}",
        "@Controller(\n  'users'\n) class A {}",
        "@Controller(\n  'users',\n) class A {}",
        "@Controller(\"users\") class A {}",
        "@Controller(`users`) class A {}",
        "@Controller( /* c */ 'users' /* d */ ) class A {}",
        "// lead\n@Controller('users') // trail\nclass A {}",
        "@Controller({ path: 'users' }) class A {}",
        "@Controller({ version: '1', path: 'users', host: 'x.com' }) class A {}",
    ];
    for source in variants {
        let c = clean(source);
        assert_eq!(c[0].path, RoutePath::Static("/users".into()), "{source}");
    }
}

#[test]
fn controller_options_object_without_path_is_the_root() {
    let c = clean("@Controller({ version: '1' }) class A {}");
    assert_eq!(c[0].path, RoutePath::Static("/".into()));
}

#[test]
fn controller_path_is_normalized() {
    let c = clean("@Controller('/users/') class A {}");
    assert_eq!(c[0].path, RoutePath::Static("/users".into()));
}

#[test]
fn controller_detection_does_not_depend_on_the_class_name() {
    let c = clean("@Controller('x') export class Whatever {}");
    assert_eq!(c[0].name, "Whatever");
    // ... and a class named *Controller without the decorator is not one.
    assert!(clean("export class UsersController {}").is_empty());
}

#[test]
fn export_forms_are_supported() {
    let c = clean(
        "@Controller('a') class A {}\n\
         @Controller('b') export class B {}\n\
         @Controller('c') export default class C {}",
    );
    let names: Vec<_> = c.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["A", "B", "C"]);
}

#[test]
fn decorator_before_export_keyword_is_supported() {
    let c = clean("@Controller('a')\nexport class A {}");
    assert_eq!(c.len(), 1);
}

#[test]
fn multiple_controllers_in_one_file_keep_source_order() {
    let c = clean(
        "@Controller('users')\nexport class UsersController { @Get() a() {} }\n\
         @Controller('admin/users')\nexport class AdminUsersController { @Get() b() {} }",
    );
    assert_eq!(c.len(), 2);
    assert_eq!(c[0].name, "UsersController");
    assert_eq!(c[1].name, "AdminUsersController");
    assert_eq!(
        routes(&c),
        pairs(&[("GET", "/users"), ("GET", "/admin/users")])
    );
}

#[test]
fn anonymous_controller_class_is_skipped_with_a_warning() {
    let a = analyze("@Controller('x') export default class {}");
    assert!(a.controllers.is_empty());
    assert_eq!(a.diagnostics.len(), 1);
    assert_eq!(a.diagnostics[0].severity, Severity::Warning);
    assert!(a.diagnostics[0].message.contains("anonymous"));
}

#[test]
fn non_call_controller_decorator_is_not_a_controller() {
    assert!(clean("@Controller class A {}").is_empty());
}

#[test]
fn nested_classes_and_other_statements_are_ignored() {
    let c = clean(
        "import { Controller } from '@nestjs/common';\n\
         interface I { a: string }\n\
         type T = { b: number };\n\
         enum E { A, B }\n\
         function f() { @Controller('hidden') class Inner {} }\n\
         export const x = 1;\n\
         @Controller('real') export class R {}",
    );
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].name, "R");
}

// ------------------------------------------------------------ http methods

#[test]
fn every_http_method_decorator_is_extracted() {
    let source = "@Controller('r') class C {\n\
        @Get() a() {}\n @Post() b() {}\n @Put() c() {}\n @Patch() d() {}\n\
        @Delete() e() {}\n @Head() f() {}\n @Options() g() {}\n @All() h() {}\n}";
    let c = clean(source);
    assert_eq!(
        routes(&c),
        pairs(&[
            ("GET", "/r"),
            ("POST", "/r"),
            ("PUT", "/r"),
            ("PATCH", "/r"),
            ("DELETE", "/r"),
            ("HEAD", "/r"),
            ("OPTIONS", "/r"),
            ("ALL", "/r"),
        ])
    );
    let names: Vec<_> = c[0].routes.iter().map(|r| r.method_name.as_str()).collect();
    assert_eq!(names, ["a", "b", "c", "d", "e", "f", "g", "h"]);
}

#[test]
fn route_paths_from_the_specification() {
    let cases = [
        ("@Get()", "/users"),
        ("@Get(':id')", "/users/:id"),
        ("@Get('profile')", "/users/profile"),
        ("@Post('login')", "/users/login"),
        ("@Put(':id')", "/users/:id"),
        ("@Patch(':id')", "/users/:id"),
        ("@Delete(':id')", "/users/:id"),
        ("@Head('health')", "/users/health"),
        ("@Options()", "/users"),
        ("@All(':id')", "/users/:id"),
        ("@Get(':id/posts')", "/users/:id/posts"),
        ("@Get(':id?')", "/users/:id?"),
        ("@Get('/profile/')", "/users/profile"),
    ];
    for (decorator, expected) in cases {
        assert_eq!(
            single_route(decorator).path,
            RoutePath::Static(expected.into()),
            "{decorator}"
        );
    }
}

#[test]
fn controller_and_route_path_composition_matrix() {
    let cases = [
        ("", "", "/"),
        ("users", "", "/users"),
        ("users", ":id", "/users/:id"),
        ("/users/", "/:id/", "/users/:id"),
        ("api/users", "profile", "/api/users/profile"),
        ("", "health", "/health"),
    ];
    for (controller, route, expected) in cases {
        let source = format!("@Controller('{controller}') class C {{ @Get('{route}') h() {{}} }}");
        let c = clean(&source);
        assert_eq!(
            c[0].routes[0].path,
            RoutePath::Static(expected.into()),
            "{controller:?} + {route:?}"
        );
    }
}

#[test]
fn nest_parameter_syntax_is_preserved_verbatim() {
    assert_eq!(
        single_route("@Get(':id')").path,
        RoutePath::Static("/users/:id".into())
    );
    assert_eq!(
        single_route("@Get('files/*')").path,
        RoutePath::Static("/users/files/*".into())
    );
}

#[test]
fn route_formats_are_equivalent() {
    let variants = [
        "@Get(':id')",
        "@Get(\n  ':id',\n)",
        "@Get(\n    ':id'\n)",
        "// Get the user\n  @Get(':id')",
        "/* doc */ @Get( /* path */ ':id')",
        "@Get(`:id`)",
        "@Get(\":id\")",
    ];
    let expected = single_route("@Get(':id')");
    for decorators in variants {
        let route = single_route(decorators);
        assert_eq!(route.path, expected.path, "{decorators}");
        assert_eq!(route.http_method, expected.http_method, "{decorators}");
        assert_eq!(route.method_name, "handler", "{decorators}");
    }
}

#[test]
fn full_crud_controller() {
    let c = clean(
        "@Controller('users')\nexport class UsersController {\n\
         @Get() findAll() {}\n @Get(':id') findOne() {}\n @Post() create() {}\n\
         @Patch(':id') update() {}\n @Delete(':id') remove() {}\n}",
    );
    assert_eq!(
        routes(&c),
        pairs(&[
            ("GET", "/users"),
            ("GET", "/users/:id"),
            ("POST", "/users"),
            ("PATCH", "/users/:id"),
            ("DELETE", "/users/:id"),
        ])
    );
}

#[test]
fn string_method_names_and_async_methods_work() {
    let c = clean(
        "@Controller() class C { @Get() async 'find-all'() {} @Get('x') public async y(): Promise<void> {} }",
    );
    let names: Vec<_> = c[0].routes.iter().map(|r| r.method_name.as_str()).collect();
    assert_eq!(names, ["find-all", "y"]);
}

#[test]
fn undecorated_and_unsupported_members_are_ignored() {
    let c = clean(
        "@Controller('u') class C {\n\
         constructor(private readonly s: S) {}\n\
         private helper() {}\n\
         @Get() static s() {}\n\
         get prop() { return 1; }\n\
         x = 1;\n\
         @Get('ok') ok() {}\n}",
    );
    assert_eq!(routes(&c), pairs(&[("GET", "/u/ok")]));
}

// ------------------------------------------------------------ parameters

fn parameters(signature: &str) -> Vec<RouteParameter> {
    let source = format!("@Controller() class C {{ @Get(':id') h({signature}) {{}} }}");
    clean(&source).remove(0).routes.remove(0).parameters
}

fn param_with_type(
    kind: ParameterKind,
    name: ParameterName,
    index: usize,
    type_name: &str,
) -> RouteParameter {
    RouteParameter {
        kind,
        name,
        index,
        type_name: Some(type_name.to_owned()),
    }
}

fn named(name: &str) -> ParameterName {
    ParameterName::Named(name.to_owned())
}

#[test]
fn every_parameter_decorator_form_is_extracted() {
    use ParameterKind::{Body, Headers, Param, Query};
    use ParameterName::Unnamed;
    let cases = [
        ("@Param() p: P", param_with_type(Param, Unnamed, 0, "P")),
        (
            "@Param('id') id: string",
            param_with_type(Param, named("id"), 0, "string"),
        ),
        ("@Query() q: Q", param_with_type(Query, Unnamed, 0, "Q")),
        (
            "@Query('page') page: number",
            param_with_type(Query, named("page"), 0, "number"),
        ),
        ("@Body() b: Dto", param_with_type(Body, Unnamed, 0, "Dto")),
        (
            "@Body('name') n: string",
            param_with_type(Body, named("name"), 0, "string"),
        ),
        ("@Headers() h: H", param_with_type(Headers, Unnamed, 0, "H")),
        (
            "@Headers('authorization') a: string",
            param_with_type(Headers, named("authorization"), 0, "string"),
        ),
    ];
    for (signature, expected) in cases {
        assert_eq!(parameters(signature), vec![expected], "{signature}");
    }
}

#[test]
fn parameter_indexes_count_undecorated_parameters() {
    use ParameterKind::{Body, Param, Query};
    let p = parameters(
        "@Param('id') id: string,\n @Query('include') include: string,\n plain: number,\n @Body() body: Dto,",
    );
    assert_eq!(
        p,
        vec![
            param_with_type(Param, named("id"), 0, "string"),
            param_with_type(Query, named("include"), 1, "string"),
            param_with_type(Body, ParameterName::Unnamed, 3, "Dto"),
        ]
    );
}

#[test]
fn unsupported_parameter_decorators_are_ignored() {
    let p = parameters(
        "@Req() req: Request, @Res() res: Response, @Ip() ip: string, @Param('id') id: string",
    );
    assert_eq!(
        p,
        vec![param_with_type(
            ParameterKind::Param,
            named("id"),
            3,
            "string"
        )]
    );
}

#[test]
fn pipes_and_extra_arguments_do_not_change_the_name() {
    use ParameterKind::{Body, Param, Query};
    let p = parameters(
        "@Param('id', ParseIntPipe) id: number, @Query('p', new DefaultValuePipe(1)) p: number, @Body(new ValidationPipe({ whitelist: true })) b: Dto",
    );
    assert_eq!(
        p,
        vec![
            param_with_type(Param, named("id"), 0, "number"),
            param_with_type(Query, named("p"), 1, "number"),
            param_with_type(Body, ParameterName::Unnamed, 2, "Dto"),
        ]
    );
}

#[test]
fn optional_default_and_typed_parameters_are_fine() {
    let p =
        parameters("@Query('a') a?: string, @Query('b') b: string = 'x', @Body() { x, y }: Dto");
    assert_eq!(p.len(), 3);
    assert_eq!(p[2].index, 2);
}

#[test]
fn several_decorators_on_one_parameter_are_all_recorded_in_order() {
    let p = parameters("@Query('a') @Param('b') both: string");
    assert_eq!(
        p,
        vec![
            param_with_type(ParameterKind::Query, named("a"), 0, "string"),
            param_with_type(ParameterKind::Param, named("b"), 0, "string"),
        ]
    );
}

// ------------------------------------------------------------ parameter types

#[test]
fn keyword_types_are_extracted() {
    let cases = [
        (
            "@Query('p') p: string",
            param_with_type(ParameterKind::Query, named("p"), 0, "string"),
        ),
        (
            "@Query('p') p: number",
            param_with_type(ParameterKind::Query, named("p"), 0, "number"),
        ),
        (
            "@Query('p') p: boolean",
            param_with_type(ParameterKind::Query, named("p"), 0, "boolean"),
        ),
    ];
    for (signature, expected) in cases {
        assert_eq!(parameters(signature), vec![expected], "{signature}");
    }
}

#[test]
fn bare_identifier_types_are_extracted() {
    let p = parameters("@Body() b: CreateUserDto");
    assert_eq!(p[0].type_name, Some("CreateUserDto".to_owned()));
}

#[test]
fn generic_types_are_not_extracted() {
    let p = parameters("@Query('p') p: Promise<string>");
    assert_eq!(p[0].type_name, None);
}

#[test]
fn array_types_are_not_extracted() {
    let p = parameters("@Query('p') p: string[]");
    assert_eq!(p[0].type_name, None);
}

#[test]
fn union_types_are_not_extracted() {
    let p = parameters("@Query('p') p: string | number");
    assert_eq!(p[0].type_name, None);
}

#[test]
fn qualified_names_are_not_extracted() {
    let p = parameters("@Query('p') p: Namespace.Type");
    assert_eq!(p[0].type_name, None);
}

#[test]
fn parenthesized_types_are_not_extracted() {
    let p = parameters("@Query('p') p: (string)");
    assert_eq!(p[0].type_name, None);
}

#[test]
fn type_with_type_arguments_is_not_extracted() {
    let p = parameters("@Query('p') p: Array<string>");
    assert_eq!(p[0].type_name, None);
}

// ------------------------------------------------------------ DTO property extraction

#[test]
fn class_dto_properties_are_extracted_in_source_order() {
    let source = r#"
class CreateUserDto {
  name: string;
  email: string;
  age: number;
}
@Controller()
class C { @Post() create(@Body() dto: CreateUserDto) {} }
"#;
    let c = clean(source);
    assert_eq!(
        c[0].routes[0].body,
        BodyShape::Fields(vec!["name".into(), "email".into(), "age".into()])
    );
}

#[test]
fn interface_dto_properties_are_extracted() {
    let source = r#"
interface UpdateUserDto {
  name?: string;
  email?: string;
}
@Controller()
class C { @Patch() update(@Body() dto: UpdateUserDto) {} }
"#;
    let c = clean(source);
    assert_eq!(
        c[0].routes[0].body,
        BodyShape::Fields(vec!["name".into(), "email".into()])
    );
}

#[test]
fn computed_property_names_are_skipped() {
    let source = r#"
class Dto {
  [key: string]: string;
  name: string;
}
@Controller()
class C { @Post() create(@Body() dto: Dto) {} }
"#;
    let c = clean(source);
    assert_eq!(c[0].routes[0].body, BodyShape::Fields(vec!["name".into()]));
}

#[test]
fn methods_and_constructors_are_ignored() {
    let source = r#"
class Dto {
  name: string;
  constructor() {}
  method() {}
  get prop() { return 1; }
}
@Controller()
class C { @Post() create(@Body() dto: Dto) {} }
"#;
    let c = clean(source);
    assert_eq!(c[0].routes[0].body, BodyShape::Fields(vec!["name".into()]));
}

#[test]
fn dto_property_limit_is_enforced() {
    let mut props = String::new();
    for i in 0..70 {
        props.push_str(&format!("  prop{}: string;\n", i));
    }
    let source = format!(
        "@Controller() class C {{ @Post() create(@Body() dto: Dto) {{}} }}\nclass Dto {{\n{props}}}"
    );
    let c = clean(&source);
    let fields = match &c[0].routes[0].body {
        BodyShape::Fields(f) => f,
        _ => panic!("expected Fields"),
    };
    assert_eq!(fields.len(), 64);
}

#[test]
fn unknown_dto_type_means_unknown_body_shape() {
    let source = r#"
@Controller()
class C { @Post() create(@Body() dto: UnknownDto) {} }
"#;
    let c = clean(source);
    assert_eq!(c[0].routes[0].body, BodyShape::Unknown);
}

#[test]
fn no_body_parameter_means_none() {
    let c = clean("@Controller() class C { @Get() findAll() {} }");
    assert_eq!(c[0].routes[0].body, BodyShape::None);
}

#[test]
fn body_without_type_annotation_means_unknown() {
    let source = r#"
@Controller()
class C { @Post() create(@Body() dto) {} }
"#;
    let c = clean(source);
    assert_eq!(c[0].routes[0].body, BodyShape::Unknown);
}

#[test]
fn named_body_parameter_combines_with_dto_fields() {
    let source = r#"
class CreateUserDto {
  name: string;
  email: string;
}
@Controller()
class C { @Post() create(@Body('name') name: string, @Body() dto: CreateUserDto) {} }
"#;
    let c = clean(source);
    // DTO fields + named body param should be combined
    assert_eq!(
        c[0].routes[0].body,
        BodyShape::Fields(vec!["name".into(), "email".into()])
    );
}

#[test]
fn dynamic_parameter_names_are_unresolved_with_a_warning() {
    let a = analyze("@Controller() class C { @Get() h(@Param(KEY) id: string) {} }");
    let p = &a.controllers[0].routes[0].parameters;
    assert_eq!(p[0].name, ParameterName::Unresolved);
    assert_eq!(a.diagnostics.len(), 1);
    assert!(a.diagnostics[0].message.contains("@Param"));
}

// ------------------------------------------------------------ unsupported

#[test]
fn unsupported_decorators_are_ignored() {
    let c = clean(
        "@ApiTags('Users')\n@Controller('users')\nexport class C {\n\
         @UseGuards(AuthGuard)\n @Roles('admin')\n @ApiTags('Users')\n @Get()\n findAll() {}\n}",
    );
    assert_eq!(routes(&c), pairs(&[("GET", "/users")]));
}

#[test]
fn custom_route_decorators_are_not_recognized() {
    let c = clean("@Controller('users') class C { @MyGet('/users') h() {} @Get('ok') ok() {} }");
    assert_eq!(routes(&c), pairs(&[("GET", "/users/ok")]));
}

#[test]
fn aliased_and_namespaced_decorators_are_not_recognized() {
    let c = clean(
        "@C('a') class A { @G() h() {} }\n\
         @common.Controller('b') class B { @common.Get() h() {} }",
    );
    assert!(c.is_empty());
}

#[test]
fn route_decorators_outside_controllers_are_ignored() {
    assert!(clean("class Plain { @Get() h() {} }").is_empty());
}

#[test]
fn dynamic_controller_path_leaves_routes_unresolved() {
    for source in [
        "const prefix = 'x';\n@Controller(prefix) class C { @Get('a') h() {} }",
        "@Controller(getPrefix()) class C { @Get('a') h() {} }",
        "@Controller('a' + 'b') class C { @Get('a') h() {} }",
        "@Controller(`a${b}`) class C { @Get('a') h() {} }",
        "@Controller(PREFIX as const) class C { @Get('a') h() {} }",
        "@Controller(('a')) class C { @Get('a') h() {} }",
        "@Controller(['a', 'b']) class C { @Get('a') h() {} }",
        "@Controller({ path: prefix }) class C { @Get('a') h() {} }",
        "@Controller({ ...opts }) class C { @Get('a') h() {} }",
        "@Controller({ [key]: 'a' }) class C { @Get('a') h() {} }",
        "@Controller(...args) class C { @Get('a') h() {} }",
    ] {
        let a = analyze(source);
        let c = &a.controllers[0];
        assert_eq!(c.path, RoutePath::Unresolved, "{source}");
        assert_eq!(c.routes[0].path, RoutePath::Unresolved, "{source}");
        assert_eq!(c.routes[0].http_method, HttpMethod::Get, "{source}");
        assert_eq!(a.diagnostics.len(), 1, "{source}: {:?}", a.diagnostics);
        assert_eq!(a.diagnostics[0].severity, Severity::Warning);
        assert!(a.diagnostics[0].message.contains('C'), "{source}");
    }
}

#[test]
fn dynamic_route_path_is_unresolved_but_siblings_are_unaffected() {
    let a = analyze(
        "@Controller('u') class C { @Get(buildPath()) a() {} @Get('b') b() {} @Get(['x','y']) c() {} }",
    );
    let r = &a.controllers[0].routes;
    assert_eq!(r[0].path, RoutePath::Unresolved);
    assert_eq!(r[1].path, RoutePath::Static("/u/b".into()));
    assert_eq!(r[2].path, RoutePath::Unresolved);
    assert_eq!(a.diagnostics.len(), 2);
}

#[test]
fn dynamic_expressions_are_never_evaluated_or_stringified() {
    // The result must not contain any text taken from the expression.
    let a = analyze("@Controller(process.env.PREFIX) class C { @Get() h() {} }");
    assert_eq!(a.controllers[0].path, RoutePath::Unresolved);
    assert!(!format!("{:?}", a.controllers).contains("PREFIX"));
}

#[test]
fn multiple_route_decorators_extract_the_first_and_warn() {
    let a = analyze("@Controller('u') class C { @Get('a') @Post('b') h() {} }");
    let r = &a.controllers[0].routes;
    assert_eq!(r.len(), 1);
    assert_eq!(
        (r[0].http_method, &r[0].path),
        (HttpMethod::Get, &RoutePath::Static("/u/a".into()))
    );
    assert_eq!(a.diagnostics.len(), 1);
    assert!(
        a.diagnostics[0]
            .message
            .contains("several route decorators")
    );
}

#[test]
fn computed_method_names_are_skipped_with_a_warning() {
    let a = analyze("@Controller() class C { @Get() [name]() {} @Get() ['lit']() {} }");
    let r = &a.controllers[0].routes;
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].method_name, "lit");
    assert_eq!(a.diagnostics.len(), 1);
}

// ------------------------------------------------------------ syntax

#[test]
fn surrounding_typescript_syntax_does_not_interfere() {
    let source = r#"
import { Controller, Get, Param } from '@nestjs/common';
import type { User } from './user';

export interface Page<T> { items: T[]; next?: string }
export type Id = string | number;
export enum Role { Admin = 'admin', User = 'user' }
declare const process: any;

abstract class Base<T extends object> {
  protected abstract load(): Promise<T>;
}

@Controller('users')
export class UsersController<T = unknown> extends Base<User> implements Marker {
  private readonly cache = new Map<string, User>();
  static instances = 0;
  constructor(private readonly service: UsersService, @Inject(TOKEN) public tok: string) { super(); }

  @Get(':id')
  public async findOne<K extends keyof User>(@Param('id') id: string, flag?: boolean): Promise<Page<User>> {
    const x = (a: number): number => a * 2;
    return { items: [] } as Page<User>;
  }

  protected abstract hidden(): void;
  protected async load(): Promise<any> { return null; }
}
"#;
    let a = analyze(source);
    assert!(a.diagnostics.is_empty(), "{:?}", a.diagnostics);
    assert_eq!(routes(&a.controllers), pairs(&[("GET", "/users/:id")]));
    assert_eq!(
        a.controllers[0].routes[0].parameters,
        vec![param_with_type(
            ParameterKind::Param,
            named("id"),
            0,
            "string"
        )]
    );
}

#[test]
fn empty_and_comment_only_files_have_no_controllers() {
    assert_eq!(analyze(""), FileAnalysis::default());
    assert_eq!(analyze("// nothing\n/* here */"), FileAnalysis::default());
}

#[test]
fn crlf_and_unicode_sources_are_handled() {
    let source = "// ключ\r\n@Controller('ü')\r\nclass C {\r\n  @Get('ж') h() {}\r\n}\r\n";
    let c = clean(source);
    assert_eq!(c[0].routes[0].path, RoutePath::Static("/ü/ж".into()));
    assert_eq!(
        (c[0].routes[0].source.line, c[0].routes[0].source.column),
        (4, 3)
    );
}

// ------------------------------------------------------------ locations

#[test]
fn locations_point_at_the_decorators() {
    let source = "import x from 'y';\n\n@Controller('users')\nexport class C {\n  @Get(':id')\n  findOne() {}\n\n\t@Post()\n  create() {}\n}\n";
    let c = clean(source);
    let file: &Path = Path::new(FILE);
    assert_eq!(&*c[0].source.file, file);
    assert_eq!((c[0].source.line, c[0].source.column), (3, 1));
    assert_eq!(
        (c[0].routes[0].source.line, c[0].routes[0].source.column),
        (5, 3)
    );
    // A tab counts as one column.
    assert_eq!(
        (c[0].routes[1].source.line, c[0].routes[1].source.column),
        (8, 2)
    );
    assert_eq!(c[0].routes[0].source.to_string(), format!("{FILE}:5:3"));
}

#[test]
fn columns_count_characters_not_bytes() {
    let c = clean("/* ééé */ @Controller() class C { @Get() h() {} }");
    assert_eq!(c[0].source.column, 11);
}

#[test]
fn line_index_handles_all_line_endings() {
    let source = "a\nb\r\nc\rd";
    let index = LineIndex::new(source);
    let at = |needle: &str| {
        let offset = u32::try_from(source.find(needle).unwrap()).unwrap();
        index.locate(source, offset)
    };
    assert_eq!(at("a"), (1, 1));
    assert_eq!(at("b"), (2, 1));
    assert_eq!(at("c"), (3, 1));
    assert_eq!(at("d"), (4, 1));
    // An offset past the end must not panic.
    assert_eq!(index.locate(source, 999).0, 4);
}

// ------------------------------------------------------------ malformed

#[test]
fn malformed_source_yields_an_error_diagnostic_with_a_position() {
    let a = extract_controllers(
        Path::new("src/bad.controller.ts"),
        "@Controller('x')\nexport class C {\n  @Get(\n  h() {}\n",
        None,
    );
    assert!(a.controllers.is_empty());
    assert_eq!(a.diagnostics.len(), 1);
    let d = &a.diagnostics[0];
    assert_eq!(d.severity, Severity::Error);
    assert_eq!(d.path.as_deref(), Some(Path::new("src/bad.controller.ts")));
    assert!(d.position.is_some());
    assert!(d.message.contains("unable to parse"));
    assert!(d.message.contains("no routes were extracted"));
}

#[test]
fn recovered_ast_is_never_used_for_partially_broken_files() {
    // The first controller is valid; the file as a whole is not.
    let a = analyze("@Controller('a') class A { @Get() ok() {} }\nclass B { @Get( }");
    assert!(a.controllers.is_empty());
    assert_eq!(a.diagnostics[0].severity, Severity::Error);
}

#[test]
fn assorted_garbage_never_panics() {
    for source in [
        "\0\0\0",
        "@",
        "@@@@",
        "@Controller(",
        "class",
        "@Controller('x') class",
        "export default",
        "}}}}}",
        "\u{feff}@Controller() class C {}",
        "`unterminated",
        "/* unterminated",
        "@Controller('x') class C { @Get() h(@Param(",
    ] {
        let _ = analyze(source);
    }
}

#[test]
fn byte_order_mark_is_accepted() {
    let c = clean("\u{feff}@Controller('x') class C {}");
    assert_eq!(c.len(), 1);
}

// ------------------------------------------------------------ determinism

#[test]
fn analyzing_the_same_source_twice_gives_identical_results() {
    let source = "@Controller('a') class A { @Get() x() {} @Post(dyn()) y(@Body() b: B) {} }\n@Controller(p) class B { @Get() z() {} }";
    assert_eq!(analyze(source), analyze(source));
}

proptest! {
    #[test]
    fn arbitrary_text_never_panics_and_is_deterministic(source in "\\PC{0,300}") {
        let first = analyze(&source);
        prop_assert_eq!(&first, &analyze(&source));
        // A file either yields an error diagnostic and no controllers, or no error.
        let failed = first.diagnostics.iter().any(|d| d.severity == Severity::Error);
        prop_assert!(!failed || first.controllers.is_empty());
    }

    #[test]
    fn mutated_valid_source_never_panics(cut in 0usize..200, insert in "[@(){}',;:\\[\\] a-zA-Z]{0,6}") {
        let base = "@Controller('users') export class C { @Get(':id') f(@Param('id') id: string) {} @Post() g() {} }";
        let cut = cut.min(base.len());
        let mut mutated = String::from(&base[..cut]);
        mutated.push_str(&insert);
        mutated.push_str(&base[cut..]);
        let _ = analyze(&mutated);
    }

    #[test]
    fn generated_routes_round_trip(
        controller in "[a-z]{0,6}(/[a-z]{1,4}){0,2}",
        route in "(:?[a-z]{1,5}){0,1}(/:?[a-z]{1,5}){0,2}",
    ) {
        let source = format!("@Controller('{controller}') class C {{ @Get('{route}') h() {{}} }}");
        let a = analyze(&source);
        prop_assert!(a.diagnostics.is_empty());
        prop_assert_eq!(
            &a.controllers[0].routes[0].path,
            &RoutePath::Static(compose_path(&[&controller, &route]))
        );
    }
}

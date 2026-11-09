# Routes: global prefix and versioning

Generated request URLs are the **effective** URLs of the running
application: the static global prefix and URI-versioning segments are
composed into every path, read from the application bootstrap. Nothing is
executed — the bootstrap file is parsed statically like every other source.

## Where the bootstrap comes from

By default the first conventional candidate that exists is used:
`src/main.ts`, then `main.ts`. Point the optional `bootstrap` key in
`nestbruno.toml` at a different entry file (see
[Configuration](/guide/configuration)). The search never scans the
repository. A missing conventional file is normal — no prefix, no
versioning is applied.

## Global prefix

```ts
// src/main.ts
async function bootstrap() {
  const app = await NestFactory.create(AppModule);
  app.setGlobalPrefix('api');
  await app.listen(3000);
}
```

`@Controller('users')` + `@Get(':id')` then generates `GET {{baseUrl}}/api/users/:id`.

### Exclude rules

```ts
app.setGlobalPrefix('api', {
  exclude: ['health', { path: 'admin', method: RequestMethod.GET }, 'legacy*'],
});
```

Supported: exact paths, trailing-`*` wildcards, plain strings and
`{ path, method }` objects (the request method is known per route, so
method-restricted excludes match exactly). Excludes apply to the path
**without** the prefix or version — NestJS semantics.

Conservative rule: a pattern that cannot be matched statically (leading
`*`, `:param`, `?`, dynamic expressions) disables the prefix entirely with
a warning. A missing prefix is honest; a wrong URL is not.

## URI versioning

```ts
import { VersioningType } from '@nestjs/common';

app.enableVersioning({
  type: VersioningType.URI,
  defaultVersion: '1',
});
```

Effective URLs become `/{{prefix}}/v{n}/{controller}/{method}` — the
version segment sits between the prefix and the controller path.

Precedence follows NestJS: a method-level `@Version(...)` overrides the
controller-level `version`, which overrides the application
`defaultVersion`.

```ts
@Controller({ path: 'users', version: '2' })
export class UsersController {
  @Get()
  findAll() {} // /api/v2/users

  @Version('3')
  @Post()
  create() {} // /api/v3/users

  @Version(VERSION_NEUTRAL)
  @Get('health')
  health() {} // /api/users/health — no version segment
}
```

- `VERSION_NEUTRAL` at any level removes the version segment; it is
  recognized only when imported from `@nestjs/common`.
- Multiple declared versions (`version: ['1', '2']`) produce **one request
  per version**; the requests are distinguished by collision-suffixed file
  names.

## What is not rendered

- **HEADER and MEDIA_TYPE versioning** are identified and reported (a
  warning explains that they do not appear in URLs), but no version segment
  is generated.
- Dynamic expressions (`process.env.PREFIX`, template values) and
  unrecognized `VersioningType` members apply nothing and warn.
- `RouterModule` composition and per-app multi-prefix workspaces are out of
  scope.

Projects without a bootstrap configuration produce exactly the same output
as before this analysis existed.

## Sync behavior

Route identity is computed from the effective path: adding or changing a
prefix or version updates the affected requests through the normal
matching tiers — an unchanged handler with a changed URL is an update or a
move, never a duplicate. Watch mode: editing controller or DTO files
re-syncs as usual; editing the bootstrap file itself needs a restart
(tracked in the limitations list).

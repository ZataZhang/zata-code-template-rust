# Accounts module host example

This is a separate React application that consumes the actual packed `@tpl/accounts-ui` npm archive. It does not import the package source or share a pnpm workspace with the public site.

From the repository root:

```bash
pnpm --dir packages/accounts-ui install --frozen-lockfile
pnpm --dir packages/accounts-ui run pack:tarball
cd examples/accounts-host
pnpm install --frozen-lockfile
pnpm build
```

The package archive is generated at `packages/accounts-ui/tpl-accounts-ui-0.1.0.tgz` and is ignored by Git. The example pins that local archive in its `package.json` and `pnpm-lock.yaml`, so run the pack command before installing the example.

## Local integration flow

For local browser integration, reuse the shared PostgreSQL and Redis containers from the sibling `../zata-ops` checkout. This avoids starting another database and Redis pair with `just accounts-up`.

From the repository root, start the shared middleware and ensure this project's database exists:

```bash
(cd ../zata-ops && just testing up postgres redis)
if ! docker exec test-postgres psql -U testuser -d postgres -tAc \
  "SELECT 1 FROM pg_database WHERE datname = 'rust_study_accounts'" | grep -qx 1; then
  docker exec test-postgres createdb -U testuser rust_study_accounts
fi
```

The `rust_study_accounts` database is already created in the current local environment. The block is safe to rerun and creates it only when missing. The local test credentials are defined in `../zata-ops/local/docker-compose.testing.yml`; set `DATABASE_URL` to that dedicated database and `REDIS_URL` to logical database 1 in the repository-root `.env.local`.

From the rust-study repository root, start the full local stack:

```bash
just run
```

`just run` installs the frontend dependencies, builds the accounts UI tarball, starts the Rust API plus the public site, admin site, and this example host, then prints their URLs. Open the **Accounts UI example** URL (`http://127.0.0.1:4174`); port 8080 is the API and its root path returns 404. Stop the local processes with `just down`. To validate package types and production builds separately, run `just accounts-ui-check`.

Browser requests stay same-origin and use the account service's ordinary-user HttpOnly cookie. Administrator credentials and cookies are not used by this package. Use `just run docker` when specifically validating the Docker service stack.

Run the browser test only against a dedicated test environment, never production. It creates a new account and leaves it in the database when run standalone. Start the account service and set `HOST_URL` (and optionally `CHROME_PATH`, `SHOT_DIR`):

```bash
cd examples/accounts-host
HOST_URL=http://127.0.0.1:4174 pnpm e2e
```

The test creates a fresh account, reads its session through a new `/auth/me` request, logs out, checks that a fresh session query returns 401, and confirms a wrong password is rejected. `packages/accounts-ui/README.md` documents the component and client API.

import { readFile, mkdir, writeFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { parse } from 'yaml'

const source = new URL('../../docs/api/openapi.yaml', import.meta.url)
const destination = new URL('../src/api/generated/contract.json', import.meta.url)
const document = parse(await readFile(source, 'utf8'))
if (document.info.version !== '2.0.0') throw new Error('API契約のバージョンを確認してください。')
// OpenAPI 3.1 schemas are JSON Schema 2020-12. Preserve their constraints and
// only relocate references, so runtime and generated TS types have one source.
const schemas = JSON.parse(JSON.stringify(document.components.schemas).replaceAll('#/components/schemas/', '#/$defs/'))
const contract = { $schema: 'https://json-schema.org/draft/2020-12/schema', $id: 'https://accilessnavi.local/api-contract', $defs: schemas }
await mkdir(new URL('../src/api/generated/', import.meta.url), { recursive: true })
await writeFile(destination, `${JSON.stringify(contract, null, 2)}\n`)
console.log(`Generated ${fileURLToPath(destination)} from docs/api/openapi.yaml`)

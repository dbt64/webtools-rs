import Ajv2020 from 'ajv/dist/2020.js'

// Compiles JSON Schema locally. No external schema loader or remote references are enabled.
const schemas = new Ajv2020({ ownProperties: true, validateFormats: false, addUsedSchema: false })

export function compileToolSchema(schema: Readonly<Record<string, unknown>>) {
  try { return schemas.compile(schema) }
  finally { schemas.removeSchema(schema) }
}

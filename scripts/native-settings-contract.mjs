// Desktop-only extension; frozen Go DTOs and storage entity layouts stay intact.
export function extendWorkspaceApprovalSettings(contract) {
  const resolve = schema => schema.$ref
    ? contract.$defs[schema.$ref.split('/').at(-1)]
    : schema;
  const schemas = [
    resolve(contract.methods['ui.setSettings'].properties.patch),
    resolve(contract.results['ui.settings']),
    resolve(contract.results['ui.setSettings']),
  ];
  for (const schema of schemas) {
    if (schema.type !== 'object' || !schema.properties) {
      throw new Error('Expected exact settings object contract');
    }
    schema.properties.approval_bypass = {type:'boolean'};
  }
  return schemas;
}

// Authentication facts only. TypeDB owns all tenant roles and authorization.
exports.onExecutePostLogin = async (event, api) => {
  if (event.resource_server?.identifier !== 'https://business.jitpomi.com/development/api') return;
  const ns = 'https://jitpomi.com/';
  if (event.user.email) api.accessToken.setCustomClaim(ns + 'email', event.user.email);
  api.accessToken.setCustomClaim(ns + 'email_verified', event.user.email_verified === true);
  const methods = event.authentication?.methods || [];
  const names = [...new Set(methods.map(m => m.name).filter(n => typeof n === 'string'))];
  if (names.length) api.accessToken.setCustomClaim(ns + 'amr', names);
  // Use primary authentication time; an MFA challenge must not refresh an old primary login.
  const primary = methods.filter(m => m.name !== 'mfa').map(m => Date.parse(m.timestamp)).filter(Number.isFinite);
  if (primary.length) api.accessToken.setCustomClaim(ns + 'auth_time', Math.floor(Math.max(...primary) / 1000));
};

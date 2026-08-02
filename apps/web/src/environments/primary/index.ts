export {
  getPrimaryKnownEnvironment,
  readPrimaryEnvironmentDescriptor,
  resetPrimaryEnvironmentDescriptorForTests,
  resolveInitialPrimaryEnvironmentDescriptor,
  writePrimaryEnvironmentDescriptor,
  __resetPrimaryEnvironmentBootstrapForTests,
  __resetPrimaryEnvironmentDescriptorBootstrapForTests,
} from "./context";

export {
  resolveInitialPrimaryEnvironmentDescriptor as ensurePrimaryEnvironmentReady,
  writePrimaryEnvironmentDescriptor as updatePrimaryEnvironmentDescriptor,
} from "./context";

export {
  createServerPairingCredential,
  forgetExternalTunnelEndpoint,
  fetchSessionState,
  isPrimaryEnvironmentPairingCredentialRejectedError,
  isPrimaryEnvironmentRequestError,
  listServerClientSessions,
  listServerPairingLinks,
  readExternalTunnelEndpoint,
  registerExternalTunnelEndpoint,
  peekPairingTokenFromUrl,
  PrimaryEnvironmentPairingCredentialRejectedError,
  PrimaryEnvironmentRequestError,
  reauthenticatePrimaryEnvironment,
  resolveInitialServerAuthGateState,
  revokeOtherServerClientSessions,
  revokeServerClientSession,
  revokeServerPairingLink,
  testExternalTunnelEndpoint,
  stripPairingTokenFromUrl,
  submitServerAuthCredential,
  takePairingTokenFromUrl,
  type ServerClientSessionRecord,
  type ServerPairingLinkRecord,
  __resetServerAuthBootstrapForTests,
} from "./auth";

export { refreshPrimarySessionState, usePrimarySessionState } from "./sessionState";

export { PrimaryEnvironmentHttpClient } from "./httpClient";

export {
  DesktopEnvironmentBootstrapIncompleteError,
  isDesktopEnvironmentBootstrapIncompleteError,
  isPrimaryEnvironmentProtocolUnsupportedError,
  isPrimaryEnvironmentUrlInvalidError,
  PrimaryEnvironmentProtocolUnsupportedError,
  PrimaryEnvironmentUrlInvalidError,
  readPrimaryEnvironmentTarget,
  resolvePrimaryEnvironmentHttpUrl,
  isLoopbackHostname,
  type PrimaryEnvironmentTarget,
} from "./target";

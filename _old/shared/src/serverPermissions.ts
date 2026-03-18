var EMPTY_SERVER_CAPABILITIES = Object.freeze({
  manageServer: false,
  manageChannels: false,
  manageAgents: false,
  manageMachines: false,
  manageMembers: false,
  changeMemberRoles: false,
  manageBilling: false,
  joinPublicChannels: false
});

var SERVER_CAPABILITY_MATRIX = {
  owner: Object.freeze({
    manageServer: true,
    manageChannels: true,
    manageAgents: true,
    manageMachines: true,
    manageMembers: true,
    changeMemberRoles: true,
    manageBilling: true,
    joinPublicChannels: true
  }),
  admin: Object.freeze({
    manageServer: true,
    manageChannels: true,
    manageAgents: true,
    manageMachines: true,
    manageMembers: true,
    changeMemberRoles: false,
    manageBilling: false,
    joinPublicChannels: true
  }),
  member: Object.freeze({
    manageServer: false,
    manageChannels: false,
    manageAgents: false,
    manageMachines: false,
    manageMembers: false,
    changeMemberRoles: false,
    manageBilling: false,
    joinPublicChannels: true
  })
};

export { EMPTY_SERVER_CAPABILITIES, SERVER_CAPABILITY_MATRIX };

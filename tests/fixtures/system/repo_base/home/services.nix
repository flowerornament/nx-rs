# nx: services and daemons
{ ... }:
{
  launchd.user.agents.test-agent = {
    command = "/usr/bin/true";
  };
}

## Broadcast 

*This is a research project / poc, please do not have any expectations towards this project.*

Broadcast is analogous to wsl.exe <exec> and doesn't really do much else, so if wsl.exe works for you, you probably don't need broadcast.

## Motivation

I wrote broadcast with the intent to learn more about terminal emulation, what the heck PTY stands for and to solve some personal issues I'd faced while using WSL <exec>.

### Problems with wsl.exe <exec>

1. Performance. Wsl.exe <exec> is slow on my machine. It can take up to 5 seconds to execute a command as simple as "wsl.exe ls", while broadcast only takes about 0:00.17s, both measured with the time command in the same directory.

2. Strange freezes with interactive commands. Sometimes wsl <exec> just freezes on my machine when running commands like perf, nano, vim, htop, etc. I have no idea why this happens, but until now I haven't had this a single time with broadcast. Maybe I'll have some time to debug this in the future and even submit a patch to wsl.

# The idea

Broadcast has a client and a server

- The server lives inside of WSL 
- The client on windows

Broadcast can be called like this: "broadcast valgrind" where valgrind is the command that should be "broadcasted" (executed) inside of WSL. 

Broadcast streams back the result without the user having to switch to WSL. Some todos remains: 

## TODO 

- Let user install aliases (broadcast r / register -c "ls -la" -a ll) so that "ll" will run "ls -la" in WSL
  For that we need to figure out what shell the user is running, powershell, nushell, cmd and need to figure out how to install aliases in these shells
- TODO configuration file
- TODO write a proper readme / blog post
- TODO Give the user an option to address specific distribution with command trough broadcast -d <distro>

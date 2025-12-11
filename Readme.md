# The idea

Broadcast has a client and a server

- The server lives inside of WSL 
- The client on windows

Broadcast can be called like this: "broadcast valgrind" where valgrind is the command that should be broadcastet to WSL. 

(Anything after broadcast should be broadcasted to WSL)

Broadcast streams back the result without the user having to switch to WSL. The question remains: 



## TODO 

- Think about how to handle interactive shells
- Let user install aliases (broadcast i -c "ls -la" -a ll) so that "ll" will run "ls -la" in WSL
  For that we need to figure out what shell the user is running, powershell, nushell, cmd and need to figure out how to install aliases in these shells

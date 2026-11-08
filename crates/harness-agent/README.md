# harness-agent

Runs a list of jobs on a bounded pool of threads. A job is one command with a working directory, environment, timeout and log file. It runs on this machine, or on another machine over `ssh -o BatchMode=yes`.

What is missing: the multi-machine scheduler. Version 0.1.0 runs every job on the host the job names, and it does not choose a host, balance load across hosts, copy files to a host, or retry a job on another host. A harness that needs those still does them itself.

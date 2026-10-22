# Where these fixtures come from

`resume-unknown-session.txt`: recorded from claude 2.1.308, `--resume` of an
unknown session. The program was started with the arguments and environment
this dialect starts it with, resuming the id
`00000000-0000-4000-8000-000000000000`, which it never kept, and handed one
`user` frame. Under `== stdout` is the one line it wrote to standard output,
unchanged: a `result` frame whose `errors` say it has no conversation by that
id. Under `== exit` is the status it exited with. It also wrote the same
sentence to standard error, which is left out, since this dialect reads the
frame.

gdb in nixpkgs is not compiled with debuginfod by default
Add 
```
set debuginfod verbose 10
set debuginfod enabled on
```
to `~/.gdbinit`
Protocol: <https://www.mankier.com/8/debuginfod#Webapi>
Disabling client cache: <https://www.mankier.com/7/debuginfod-client-config#Cache>
write 0 to `~/.cache/debuginfod_client/cache_miss_s` and `~/.cache/debuginfod_client/max_unused_age_s` and `~/.cache/debuginfod_client/cache_clean_interval_s`

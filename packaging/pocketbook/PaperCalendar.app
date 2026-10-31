#!/bin/sh
APPDIR=/mnt/ext1/applications/paper-calendar
cd "$APPDIR" || exit 1
export LD_LIBRARY_PATH="/ebrmain/lib:/usr/lib:/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
{
    echo "=== Paper Calendar start ==="
    date
    uname -a
    if command -v getconf >/dev/null 2>&1; then getconf GNU_LIBC_VERSION; fi
} >> launch.log 2>&1
if [ -f LIVE_CALDAV ]; then
    if [ -f credentials.sh ]; then
        . ./credentials.sh
    fi
    exec ./paper-calendar --config config.json --inkview >> launch.log 2>&1
else
    exec ./paper-calendar --config config.json --demo --inkview >> launch.log 2>&1
fi

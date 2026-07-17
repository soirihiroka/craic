__GRADLE_PROGRAM__ assembleDebug && \
apk_path="$(find . -type f -path '*/build/outputs/apk/debug/*.apk' | head -n 1)" && \
if [ -z "$apk_path" ]; then \
    echo "No debug APK found to install." >&2; \
    exit 1; \
fi && \
adb install -r "$apk_path" && \
component=__COMPONENT__ && \
package_name="${component%%/*}" && \
echo "Clearing logcat buffer..." && \
adb logcat -c && \
echo "Starting $component" && \
adb shell am start -W -n "$component" && \
pid="" && \
for _ in 1 2 3 4 5 6 7 8 9 10; do \
    pid="$(adb shell pidof "$package_name" 2>/dev/null | tr -d '\r' | awk '{print $1}')"; \
    if [ -n "$pid" ]; then \
        break; \
    fi; \
    sleep 0.5; \
done && \
if [ -z "$pid" ]; then \
    echo "Unable to find running Android process for $package_name." >&2; \
    exit 1; \
fi && \
echo "Attached logcat to $package_name (pid $pid). Press Ctrl-C to stop." && \
exec adb logcat --pid="$pid"

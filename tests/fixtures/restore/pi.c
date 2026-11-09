// Native Pi publisher that resumes: `pi --session <id>` publishes that id, plain `pi` a new one.
// Records its arguments, then blanks them as Pi's `process.title = "pi"` does. No model calls.
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#include <unistd.h>

int main(int argc, char **argv) {
    const char *root = getenv("PLANE_NAV_ROOT");
    const char *pane = getenv("TMUX_PANE");
    char session[512], path[4096], temporary[4100], record[4096];
    if (argc == 3 && strcmp(argv[1], "--session") == 0) {
        snprintf(session, sizeof(session), "%s", argv[2]);
    } else if (argc == 1) {
        snprintf(session, sizeof(session), "pi-session-%d", getpid());
    } else {
        return 1;
    }
    snprintf(record, sizeof(record), "%s/argv-%s", root, session);
    FILE *log = fopen(record, "a");
    if (!log) return 2;
    for (int i = 1; i < argc; i++) fprintf(log, "%s%s", i > 1 ? " " : "", argv[i]);
    fprintf(log, "\n");
    fclose(log);
    // The arguments are contiguous after argv[0]; `ps` then shows just `pi`.
    if (argc > 1) memset(argv[1], 0, (size_t)(argv[argc - 1] + strlen(argv[argc - 1]) - argv[1]));
    snprintf(path, sizeof(path), "%s/tmux-agent-plane/pi/%d.json", getenv("XDG_STATE_HOME"), getpid());
    snprintf(temporary, sizeof(temporary), "%s.tmp", path);
    printf("Pi fixture %s in %s\n", session, pane);
    fflush(stdout);
    for (;;) {
        struct timeval now;
        gettimeofday(&now, NULL);
        unsigned long long ms = (unsigned long long)now.tv_sec * 1000 + now.tv_usec / 1000;
        FILE *output = fopen(temporary, "w");
        if (!output) return 3;
        fprintf(output, "{\"version\":1,\"pid\":%d,\"pane_id\":\"%s\",\"session_id\":\"%s\",\"completion_epoch\":\"e-%d\",\"state\":\"idle\",\"detail\":null,\"title\":\"Restore fixture %s\",\"completed_turns\":0,\"updated_at\":%llu}", getpid(), pane, session, getpid(), session, ms);
        fclose(output);
        if (rename(temporary, path)) return 4;
        usleep(200000);
    }
}

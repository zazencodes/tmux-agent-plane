// Plain native Pi publisher with per-pane controls and input recording. No model calls.
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/time.h>
#include <termios.h>
#include <unistd.h>

int main(void) {
    const char *root = getenv("PLANE_NAV_ROOT");
    const char *pane = getenv("TMUX_PANE");
    char command[4096], path[4096], temporary[4100], input_path[4096];
    snprintf(command, sizeof(command), "%s/%s.command", root, pane);
    snprintf(input_path, sizeof(input_path), "%s/%s.input", root, pane);
    snprintf(path, sizeof(path), "%s/tmux-agent-plane/pi/%d.json", getenv("XDG_STATE_HOME"), getpid());
    snprintf(temporary, sizeof(temporary), "%s.tmp", path);
    struct termios terminal;
    if (tcgetattr(STDIN_FILENO, &terminal)) return 1;
    cfmakeraw(&terminal);
    if (tcsetattr(STDIN_FILENO, TCSANOW, &terminal)) return 2;
    if (fcntl(STDIN_FILENO, F_SETFL, O_NONBLOCK)) return 3;
    // Report focus like a real agent, so a focus event reaching a hidden pane shows up as input.
    printf("\033[?1004hNative Pi fixture %s ready\r\n", pane);
    fflush(stdout);
    for (;;) {
        char bytes[4096];
        ssize_t length = read(STDIN_FILENO, bytes, sizeof(bytes));
        if (length > 0) {
            FILE *log = fopen(input_path, "ab");
            if (!log) return 4;
            fwrite(bytes, 1, (size_t)length, log);
            fclose(log);
        }
        char state[32] = "working";
        unsigned long count = 0, identity = 0;
        FILE *input = fopen(command, "r");
        if (input) {
            if (fscanf(input, "%31s %lu %lu", state, &count, &identity) < 2) return 5;
            fclose(input);
        }
        struct timeval now;
        gettimeofday(&now, NULL);
        unsigned long long ms = (unsigned long long)now.tv_sec * 1000 + now.tv_usec / 1000;
        FILE *output = fopen(temporary, "w");
        if (!output) return 6;
        fprintf(output, "{\"version\":1,\"pid\":%d,\"pane_id\":\"%s\",\"session_id\":\"nav-%d-%lu\",\"completion_epoch\":\"nav-%d\",\"state\":\"%s\",\"detail\":\"Review fixture permission\",\"title\":\"Native payment review %s\",\"completed_turns\":%lu,\"updated_at\":%llu}", getpid(), pane, getpid(), identity, getpid(), state, pane, count, ms);
        fclose(output);
        if (rename(temporary, path)) return 7;
        usleep(50000);
    }
}

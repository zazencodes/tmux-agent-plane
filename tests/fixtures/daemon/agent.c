// Native-format publisher fixture: no agent CLI, credentials or model requests.
#include <stdio.h>
#include <fcntl.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#include <termios.h>
#include <unistd.h>

int main(int argc, char **argv) {
    (void)argc;
    const char *kind = strrchr(argv[0], '/');
    kind = kind ? kind + 1 : argv[0];
    const char *root = getenv("PLANE_FIXTURE_ROOT");
    const char *pane = getenv("TMUX_PANE");
    char command[4096], path[4096], temporary[4100];
    char input_path[4096];
    const int record_input = getenv("PLANE_FIXTURE_INPUT") != NULL;
    if (record_input) {
        struct termios terminal;
        if (tcgetattr(STDIN_FILENO, &terminal) != 0) return 5;
        cfmakeraw(&terminal);
        if (tcsetattr(STDIN_FILENO, TCSANOW, &terminal) != 0) return 6;
        if (fcntl(STDIN_FILENO, F_SETFL, O_NONBLOCK) != 0) return 7;
        snprintf(input_path, sizeof(input_path), "%s/%s.input", root, kind);
        printf("\033[?1004hFixture %s input ready\r\n", kind);
        fflush(stdout);
    }
    snprintf(command, sizeof(command), "%s/%s.command", root, kind);
    if (strcmp(kind, "claude") == 0)
        snprintf(path, sizeof(path), "%s/sessions/%d.json", getenv("CLAUDE_CONFIG_DIR"), getpid());
    else
        snprintf(path, sizeof(path), "%s/tmux-agent-plane/pi/%d.json", getenv("XDG_STATE_HOME"), getpid());
    snprintf(temporary, sizeof(temporary), "%s.tmp", path);
    for (;;) {
        if (record_input) {
            char bytes[1024];
            ssize_t length = read(STDIN_FILENO, bytes, sizeof(bytes));
            if (length > 0) {
                FILE *log = fopen(input_path, "ab");
                if (!log) return 8;
                fwrite(bytes, 1, (size_t)length, log);
                fclose(log);
            }
        }
        char state[64] = "busy";
        unsigned long count = 0;
        FILE *input = fopen(command, "r");
        if (input) {
            if (fscanf(input, "%63s %lu", state, &count) < 1) return 2;
            fclose(input);
        }
        struct timeval now;
        gettimeofday(&now, NULL);
        unsigned long long ms = (unsigned long long)now.tv_sec * 1000 + now.tv_usec / 1000;
        FILE *output = fopen(temporary, "w");
        if (!output) return 3;
        if (strcmp(kind, "claude") == 0)
            fprintf(output, "{\"pid\":%d,\"kind\":\"interactive\",\"sessionId\":\"fixture-claude\",\"startedAt\":1,\"tmux\":\"scratch:@1.%s\",\"status\":\"%s\"}", getpid(), pane, state);
        else
            fprintf(output, "{\"version\":1,\"pid\":%d,\"pane_id\":\"%s\",\"session_id\":\"fixture-pi\",\"completion_epoch\":\"fixture-%d\",\"state\":\"%s\",\"title\":\"Fixture Pi\",\"completed_turns\":%lu,\"updated_at\":%llu}", getpid(), pane, getpid(), state, count, ms);
        fclose(output);
        if (rename(temporary, path) != 0) return 4;
        usleep(100000);
    }
}

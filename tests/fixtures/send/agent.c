// Native Claude session + raw terminal fixture. No model, account or agent CLI.
#include <stdio.h>
#include <stdlib.h>
#include <sys/select.h>
#include <termios.h>
#include <unistd.h>

int main(void) {
    const char *root = getenv("PLANE_SEND_ROOT");
    char command[4096], received[4096], session[4096], temporary[4100];
    snprintf(command, sizeof(command), "%s/status", root);
    snprintf(received, sizeof(received), "%s/received", root);
    snprintf(session, sizeof(session), "%s/sessions/%d.json", getenv("CLAUDE_CONFIG_DIR"), getpid());
    snprintf(temporary, sizeof(temporary), "%s.tmp", session);
    struct termios terminal;
    if (tcgetattr(STDIN_FILENO, &terminal)) return 1;
    cfmakeraw(&terminal);
    if (tcsetattr(STDIN_FILENO, TCSANOW, &terminal)) return 2;
    fputs("\033[?2004h", stdout);
    fflush(stdout);
    for (;;) {
        char state[32];
        FILE *input = fopen(command, "r");
        if (!input || fscanf(input, "%31s", state) != 1) return 3;
        fclose(input);
        FILE *output = fopen(temporary, "w");
        if (!output) return 4;
        fprintf(output, "{\"pid\":%d,\"kind\":\"interactive\",\"sessionId\":\"send-fixture\",\"startedAt\":1,\"tmux\":\"scratch:@1.%s\",\"status\":\"%s\",\"waitingFor\":\"fixture question\"}", getpid(), getenv("TMUX_PANE"), state);
        fclose(output);
        if (rename(temporary, session)) return 5;
        fd_set readers;
        FD_ZERO(&readers);
        FD_SET(STDIN_FILENO, &readers);
        struct timeval delay = {.tv_sec = 0, .tv_usec = 50000};
        if (select(STDIN_FILENO + 1, &readers, NULL, NULL, &delay) < 0) return 6;
        if (FD_ISSET(STDIN_FILENO, &readers)) {
            char bytes[4096];
            ssize_t count = read(STDIN_FILENO, bytes, sizeof(bytes));
            if (count <= 0) return 7;
            output = fopen(received, "ab");
            if (!output) return 8;
            if (fwrite(bytes, 1, (size_t)count, output) != (size_t)count) return 9;
            fclose(output);
        }
    }
}

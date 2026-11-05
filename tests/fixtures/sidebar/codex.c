// Fake Codex: an outermost `codex` with a `codex` helper nested under another process, as real
// Codex starts `codex app-server` and `codex sandbox` beneath its node_repl.
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

int main(int argc, char **argv) {
    (void)argv;
    if (argc == 1) {
        pid_t child = fork();
        if (child < 0) {
            perror("fork");
            return 1;
        }
        if (child == 0) {
            // The trailing command stops sh exec-ing the helper over itself.
            execl("/bin/sh", "sh", "-c", "codex helper; true", (char *)NULL);
            perror("execl");
            _exit(1);
        }
    } else {
        char marker[1024];
        snprintf(marker, sizeof marker, "%s/codex-helper", getenv("PLANE_NAV_ROOT"));
        FILE *file = fopen(marker, "w");
        if (!file) {
            perror(marker);
            return 1;
        }
        fclose(file);
    }
    for (;;) {
        pause();
    }
}

// A native Claude publisher that records exact argv, without a model or account.
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int main(int argc, char **argv) {
    const char *root = getenv("PLANE_SPAWN_ROOT");
    char path[4096], session[4096], exit_path[4096];
    snprintf(path, sizeof(path), "%s/argv", root);
    FILE *output = fopen(path, "wb");
    if (!output) return 1;
    for (int i = 1; i < argc; i++)
        if (fwrite(argv[i], 1, strlen(argv[i]) + 1, output) != strlen(argv[i]) + 1) return 2;
    if (fclose(output)) return 3;
    snprintf(session, sizeof(session), "%s/sessions/%d.json", getenv("CLAUDE_CONFIG_DIR"), getpid());
    output = fopen(session, "w");
    if (!output) return 4;
    fprintf(output, "{\"pid\":%d,\"kind\":\"interactive\",\"sessionId\":\"spawn-fixture\",\"startedAt\":1,\"tmux\":\"scratch:@1.%s\",\"status\":\"idle\"}", getpid(), getenv("TMUX_PANE"));
    if (fclose(output)) return 5;
    snprintf(exit_path, sizeof(exit_path), "%s/exit", root);
    while (access(exit_path, F_OK)) usleep(10000);
    if (unlink(session)) return 6;
    return 0;
}

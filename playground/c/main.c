#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define MAX_NAME 64

typedef enum {
    ROLE_VIEWER,
    ROLE_EDITOR,
    ROLE_ADMIN,
} Role;

typedef struct {
    int id;
    char name[MAX_NAME];
    Role role;
} User;

static const char *role_name(Role r) {
    switch (r) {
        case ROLE_VIEWER: return "viewer";
        case ROLE_EDITOR: return "editor";
        case ROLE_ADMIN:  return "admin";
        default:          return "unknown";
    }
}

static int user_compare_by_id(const void *a, const void *b) {
    const User *ua = (const User *)a;
    const User *ub = (const User *)b;
    return ua->id - ub->id;
}

static void print_user(const User *u) {
    printf("#%d %s (%s)\n", u->id, u->name, role_name(u->role));
}

int main(int argc, char **argv) {
    (void)argc;
    (void)argv;

    User users[] = {
        { .id = 3, .name = "Carol", .role = ROLE_ADMIN },
        { .id = 1, .name = "Alice", .role = ROLE_EDITOR },
        { .id = 2, .name = "Bob",   .role = ROLE_VIEWER },
    };
    const size_t n = sizeof(users) / sizeof(users[0]);

    qsort(users, n, sizeof(User), user_compare_by_id);

    for (size_t i = 0; i < n; ++i) {
        print_user(&users[i]);
    }

    char *greeting = malloc(128);
    if (!greeting) {
        fprintf(stderr, "malloc failed\n");
        return EXIT_FAILURE;
    }
    snprintf(greeting, 128, "Total users: %zu", n);
    printf("%s\n", greeting);
    free(greeting);

    return EXIT_SUCCESS;
}

// SQL statement logger for an instrumented COPY of rekordbox (never the installed app).
//
// Built as a drop-in `libsqlcipher.0.dylib`: it re-exports the real library (renamed to
// `libsqlcipher_real.0.dylib`) and overrides sqlite3_open / sqlite3_open_v2 to register
// sqlite3_trace_v2 on every connection. Each executed statement is appended, with bound values
// expanded, to $HOME/re-work/traces/sqltrace-<pid>.log as
//   <unix ms>\t<thread>\t<db file>\t<sql>
// See docs/re/tooling.md.

#include <dlfcn.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#include <unistd.h>

typedef struct sqlite3 sqlite3;
typedef struct sqlite3_stmt sqlite3_stmt;
#define SQLITE_TRACE_STMT 0x01

static void *real_lib;
static FILE *log_file;
static pthread_mutex_t log_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_once_t init_once = PTHREAD_ONCE_INIT;

static int (*real_open)(const char *, sqlite3 **);
static int (*real_open_v2)(const char *, sqlite3 **, int, const char *);
static int (*real_trace_v2)(sqlite3 *, unsigned, int (*)(unsigned, void *, void *, void *), void *);
static char *(*real_expanded_sql)(sqlite3_stmt *);
static void (*real_free)(void *);
static const char *(*real_db_filename)(sqlite3 *, const char *);

static void init(void) {
    Dl_info info;
    dladdr((void *)init, &info);
    char path[4096];
    snprintf(path, sizeof path, "%s", info.dli_fname);
    char *slash = strrchr(path, '/');
    snprintf(slash ? slash + 1 : path, sizeof path - (slash ? (size_t)(slash + 1 - path) : 0),
             "libsqlcipher_real.0.dylib");
    real_lib = dlopen(path, RTLD_NOW | RTLD_LOCAL);
    if (!real_lib) abort();
    real_open = dlsym(real_lib, "sqlite3_open");
    real_open_v2 = dlsym(real_lib, "sqlite3_open_v2");
    real_trace_v2 = dlsym(real_lib, "sqlite3_trace_v2");
    real_expanded_sql = dlsym(real_lib, "sqlite3_expanded_sql");
    real_free = dlsym(real_lib, "sqlite3_free");
    real_db_filename = dlsym(real_lib, "sqlite3_db_filename");

    const char *home = getenv("HOME");
    char log_path[4096];
    snprintf(log_path, sizeof log_path, "%s/re-work/traces/sqltrace-%d.log", home ? home : "/tmp", getpid());
    log_file = fopen(log_path, "a");
}

static int on_trace(unsigned type, void *ctx, void *p, void *x) {
    (void)ctx;
    (void)x;
    if (type != SQLITE_TRACE_STMT || !log_file) return 0;
    sqlite3_stmt *stmt = p;
    char *sql = real_expanded_sql(stmt);
    struct timeval tv;
    gettimeofday(&tv, NULL);
    pthread_mutex_lock(&log_lock);
    fprintf(log_file, "%lld\t%p\t%s\t%s\n", (long long)tv.tv_sec * 1000 + tv.tv_usec / 1000,
            (void *)pthread_self(), (const char *)ctx, sql ? sql : "(null)");
    fflush(log_file);
    pthread_mutex_unlock(&log_lock);
    if (sql) real_free(sql);
    return 0;
}

static void attach(sqlite3 *db, const char *filename) {
    if (!db) return;
    const char *name = strdup(filename ? filename : "?");
    real_trace_v2(db, SQLITE_TRACE_STMT, on_trace, (void *)name);
}

int sqlite3_open(const char *filename, sqlite3 **db) {
    pthread_once(&init_once, init);
    int rc = real_open(filename, db);
    if (rc == 0) attach(*db, filename);
    return rc;
}

int sqlite3_open_v2(const char *filename, sqlite3 **db, int flags, const char *vfs) {
    pthread_once(&init_once, init);
    int rc = real_open_v2(filename, db, flags, vfs);
    if (rc == 0) attach(*db, filename);
    return rc;
}

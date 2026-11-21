#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <string.h>
#include <unistd.h>
static int targeted(const char *p) { return p && strstr(p, "abcdefabcdefabcdefabcdefabcdefab.vpk") != 0; }
int unlink(const char *p) { static int (*next)(const char *); if (!next) next=dlsym(RTLD_NEXT,"unlink"); if (targeted(p)) { errno=EIO; return -1; } return next(p); }
int unlinkat(int dir, const char *p, int flags) { static int (*next)(int,const char*,int); if (!next) next=dlsym(RTLD_NEXT,"unlinkat"); if (targeted(p)) { errno=EIO; return -1; } return next(dir,p,flags); }

/* A block comment that spans
   two lines of prose. */
#include <stdio.h>

// A line comment explaining the function below.
int main(void) {
    const char *s = "// not a comment";
    printf("%s\n", s); /* trailing block */
    return 0;
}

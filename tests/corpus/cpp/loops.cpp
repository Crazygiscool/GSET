#include <iostream>

int main() {
    int total = 0;
    int i = 0;
    while (i < 5) {
        total = total + i;
        i = i + 1;
    }
    std::cout << total << std::endl;
    return 0;
}

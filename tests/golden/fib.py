"Fibonacci with annotations.\n\nThe milestone 1 gate: `gset transpile tests/corpus/py/fib.py --to go` must emit\nGo that passes `gofmt` and `go vet`. Annotated deliberately, because the IR does\nnot infer types yet, and guessing a dynamic parameter's type is exactly the\ndefect class the rewrite exists to remove.\n"

def fib(n):
    if (n < 2):
        return n
    return (fib((n - 1)) + fib((n - 2)))

print(fib(10))


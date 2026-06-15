from pydantic import BaseModel

GREETING = "hi"

class Greeter(BaseModel):           # third-party base class (third-party def target)
    name: str

    def greet(self) -> str:         # class->method `greet` — exists ONLY as a CHILD of Greeter
        return f"{GREETING} {self.name}"

    def module_greet(self) -> str:  # DUPLICATE name `module_greet` (method side; resolve_pos first-hit)
        return GREETING

def module_greet() -> str:          # DUPLICATE name `module_greet` at module scope (resolve_pos first-hit)
    return GREETING

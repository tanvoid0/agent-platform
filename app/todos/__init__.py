"""Todo board domain — personal kanban with planning agents."""

__all__ = ["router"]


def __getattr__(name: str):
    if name == "router":
        from todos.routes import router

        return router
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")

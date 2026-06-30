"""Reusable Pydantic Field definitions for consistent validation across schemas."""

from pydantic import Field

# Common string fields used across resource schemas
ResourceName = Field(min_length=1, max_length=256, description="Human-readable name")
ResourceDescription = Field(default=None, max_length=4096, description="Detailed description")
ResourceColor = Field(default=None, max_length=32, description="Hex color or CSS color name")
ResourceCategory = Field(default=None, max_length=128, description="Optional category/tag")

# Path and content fields
FilePath = Field(min_length=1, max_length=8192, description="Relative or absolute file path")
TextContent = Field(default="", description="Text content")

# ID fields
PositiveInt = Field(ge=1, description="Positive integer ID")

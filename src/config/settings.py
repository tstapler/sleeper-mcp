"""Configuration management for the Sleeper MCP server.

This module handles all configuration settings for the application, including:
- Server settings
- API endpoints
- Rate limiting parameters
- Caching configuration
"""
from functools import lru_cache
from typing import Optional

from pydantic import BaseModel, Field, HttpUrl


class SleeperAPIConfig(BaseModel):
    """Configuration for the Sleeper API."""

    base_url: HttpUrl = Field(
        default="https://api.sleeper.app/v1",
        description="Base URL for the Sleeper API",
    )
    rate_limit_per_minute: int = Field(
        default=1000,
        description="Maximum number of requests allowed per minute",
    )
    timeout_seconds: float = Field(
        default=30.0,
        description="Timeout for API requests in seconds",
    )


class CacheConfig(BaseModel):
    """Configuration for caching."""

    ttl_seconds: int = Field(
        default=300,
        description="Time to live for cached items in seconds",
    )
    max_size: int = Field(
        default=1000,
        description="Maximum number of items in the cache",
    )


class ServerConfig(BaseModel):
    """Main server configuration."""

    host: str = Field(
        default="0.0.0.0",
        description="Host to bind the server to",
    )
    port: int = Field(
        default=8000,
        description="Port to run the server on",
    )
    debug: bool = Field(
        default=False,
        description="Enable debug mode",
    )
    environment: str = Field(
        default="development",
        description="Server environment (development/production)",
    )


class Config(BaseModel):
    """Root configuration class combining all settings."""

    server: ServerConfig = Field(default_factory=ServerConfig)
    sleeper_api: SleeperAPIConfig = Field(default_factory=SleeperAPIConfig)
    cache: CacheConfig = Field(default_factory=CacheConfig)


@lru_cache()
def get_config() -> Config:
    """Get the application configuration.
    
    Returns:
        Config: Application configuration object
        
    Note:
        This function is cached to avoid reading/parsing configuration multiple times.
    """
    return Config()

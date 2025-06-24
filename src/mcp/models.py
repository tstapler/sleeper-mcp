"""MCP protocol models for the Sleeper integration.

This module defines the data models for the Machine Context Provider (MCP) protocol,
which enables integration with Goose AI.
"""

from enum import Enum
from typing import Any, Dict, List, Optional, Union

from pydantic import BaseModel, Field


class MCPFunctionParameter(BaseModel):
    """Definition of a function parameter in the MCP protocol."""

    name: str = Field(..., description="Name of the parameter")
    type: str = Field(..., description="Data type of the parameter")
    description: str = Field(..., description="Description of the parameter")
    required: bool = Field(default=False, description="Whether the parameter is required")
    default: Optional[Any] = Field(None, description="Default value for the parameter")


class MCPFunction(BaseModel):
    """Definition of a function in the MCP protocol."""

    name: str = Field(..., description="Name of the function")
    description: str = Field(..., description="Description of what the function does")
    parameters: List[MCPFunctionParameter] = Field(
        default_factory=list,
        description="List of function parameters"
    )


class MCPInvocation(BaseModel):
    """Request to invoke a function via MCP."""

    function_name: str = Field(..., description="Name of the function to invoke")
    parameters: Dict[str, Any] = Field(
        default_factory=dict,
        description="Parameters for the function invocation"
    )


class MCPResponseStatus(str, Enum):
    """Possible statuses for an MCP response."""

    SUCCESS = "success"
    ERROR = "error"


class MCPResponse(BaseModel):
    """Response from an MCP function invocation."""

    status: MCPResponseStatus = Field(..., description="Status of the invocation")
    result: Optional[Any] = Field(None, description="Result of the function invocation")
    error: Optional[str] = Field(None, description="Error message if status is error")


class MCPCapabilities(BaseModel):
    """Capabilities exposed by the MCP server."""

    functions: List[MCPFunction] = Field(
        default_factory=list,
        description="List of available functions"
    )
    version: str = Field(..., description="MCP protocol version")
    provider: str = Field(..., description="Name of the MCP provider")

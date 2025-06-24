"""Tests for fantasy football context providers."""

import pytest
from typing import Dict, Any

from src.mcp.context import (
    FantasyFootballContext,
    FantasyPosition,
    LeagueType,
    ScoringRule,
)

@pytest.fixture
def fantasy_context() -> FantasyFootballContext:
    """Create a fantasy football context provider for testing."""
    return FantasyFootballContext()


def test_get_position_info(fantasy_context: FantasyFootballContext):
    """Test retrieving position information."""
    # Test valid position
    qb_info = fantasy_context.get_position_info("QB")
    assert isinstance(qb_info, FantasyPosition)
    assert qb_info.code == "QB"
    assert qb_info.name == "Quarterback"
    assert len(qb_info.scoring_relevance) > 0

    # Test case insensitivity
    qb_info_lower = fantasy_context.get_position_info("qb")
    assert qb_info_lower == qb_info

    # Test invalid position
    assert fantasy_context.get_position_info("XX") is None


def test_get_scoring_rule(fantasy_context: FantasyFootballContext):
    """Test retrieving scoring rule information."""
    # Test valid rule
    pass_td = fantasy_context.get_scoring_rule("pass_td")
    assert isinstance(pass_td, ScoringRule)
    assert pass_td.name == "Passing Touchdown"
    assert pass_td.points == 4.0
    assert "QB" in pass_td.applies_to

    # Test case insensitivity
    pass_td_upper = fantasy_context.get_scoring_rule("PASS_TD")
    assert pass_td_upper == pass_td

    # Test invalid rule
    assert fantasy_context.get_scoring_rule("invalid_rule") is None


def test_get_league_type(fantasy_context: FantasyFootballContext):
    """Test retrieving league type information."""
    # Test valid league type
    standard = fantasy_context.get_league_type("standard")
    assert isinstance(standard, LeagueType)
    assert standard.name == "Standard League"
    assert standard.typical_settings["ppr"] == 0.0

    # Test case insensitivity
    standard_upper = fantasy_context.get_league_type("STANDARD")
    assert standard_upper == standard

    # Test invalid league type
    assert fantasy_context.get_league_type("invalid_type") is None


def test_explain_scoring(fantasy_context: FantasyFootballContext):
    """Test scoring explanation generation."""
    # Test QB scoring explanation
    qb_stats = {
        "passing_yards": 300,
        "passing_tds": 2,
    }
    explanation = fantasy_context.explain_scoring(qb_stats, "QB")
    assert "Scoring explanation for QB" in explanation
    assert "Passing yards (300)" in explanation
    assert "Passing TDs (2)" in explanation
    assert "Total points:" in explanation

    # Test invalid position
    invalid_explanation = fantasy_context.explain_scoring({}, "XX")
    assert invalid_explanation == "Unknown position"


def test_suggest_strategies(fantasy_context: FantasyFootballContext):
    """Test strategy suggestion generation."""
    # Test PPR league suggestions
    ppr_settings = {
        "scoring_settings": {
            "reception": 1.0
        }
    }
    ppr_strategies = fantasy_context.suggest_strategies(ppr_settings)
    assert any("receiver" in strategy.lower() for strategy in ppr_strategies)
    assert any("ppr" in strategy.lower() for strategy in ppr_strategies)

    # Test keeper league suggestions
    keeper_settings = {
        "keeper_settings": {
            "count": 3
        }
    }
    keeper_strategies = fantasy_context.suggest_strategies(keeper_settings)
    assert any("young players" in strategy.lower() for strategy in keeper_strategies)
    assert any("future" in strategy.lower() for strategy in keeper_strategies)

    # Test standard league suggestions
    standard_settings = {
        "scoring_settings": {},
        "keeper_settings": {}
    }
    standard_strategies = fantasy_context.suggest_strategies(standard_settings)
    assert len(standard_strategies) > 0  # Should still get general strategies


@pytest.mark.parametrize("position_code", ["QB", "RB", "WR", "TE", "K", "DEF"])
def test_all_positions_have_complete_info(
    fantasy_context: FantasyFootballContext,
    position_code: str
):
    """Test that all positions have complete information."""
    position = fantasy_context.get_position_info(position_code)
    assert position is not None
    assert position.code == position_code
    assert position.name
    assert position.description
    assert len(position.scoring_relevance) > 0
    assert position.typical_roster_spots > 0


def test_scoring_rules_consistency(fantasy_context: FantasyFootballContext):
    """Test that scoring rules are consistent and well-formed."""
    for rule_name, rule in fantasy_context.scoring_rules.items():
        assert rule.name
        assert rule.description
        assert isinstance(rule.points, float)
        assert rule.category
        assert len(rule.applies_to) > 0
        # Verify that all referenced positions exist
        for position in rule.applies_to:
            assert position in fantasy_context.positions


def test_league_types_consistency(fantasy_context: FantasyFootballContext):
    """Test that league types are consistent and well-formed."""
    for type_name, league_type in fantasy_context.league_types.items():
        assert league_type.name
        assert league_type.description
        assert isinstance(league_type.typical_settings, dict)
        assert len(league_type.common_strategies) > 0
        # Verify that roster settings are present
        assert "roster_size" in league_type.typical_settings
        assert "starters" in league_type.typical_settings

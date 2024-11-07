// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/a11y/lib/screen_reader/recover_a11y_focus_action.h"

#include <fuchsia/accessibility/cpp/fidl.h>
#include <zircon/types.h>

#include <memory>

#include <gmock/gmock.h>

#include "gtest/gtest.h"
#include "src/ui/a11y/bin/a11y_manager/tests/util/util.h"
#include "src/ui/a11y/lib/screen_reader/tests/mocks/mock_screen_reader_context.h"
#include "src/ui/a11y/lib/screen_reader/tests/screen_reader_action_test_fixture.h"
#include "src/ui/a11y/lib/semantics/tests/mocks/mock_semantic_provider.h"
#include "src/ui/a11y/lib/semantics/tests/mocks/mock_semantics_source.h"

namespace accessibility_test {
namespace {

using a11y::ScreenReaderContext;

constexpr zx_koid_t NON_EXISTENT_KOID = 999999;
constexpr uint32_t INVALID_NODE_ID = 999999;

class RecoverA11YFocusActionTest : public ScreenReaderActionTest {
 public:
  RecoverA11YFocusActionTest() = default;
  ~RecoverA11YFocusActionTest() override = default;

  void SetUp() override {
    ScreenReaderActionTest::SetUp();

    fuchsia::accessibility::semantics::Node node;
    node.set_node_id(0);
    node.set_role(fuchsia::accessibility::semantics::Role::TEXT_FIELD);
    node.mutable_child_ids()->push_back(1);

    fuchsia::accessibility::semantics::Node node2;
    node2.set_node_id(1);
    node2.mutable_attributes()->set_label("node2");

    mock_semantics_source()->CreateSemanticNode(mock_semantic_provider()->koid(), std::move(node));
    mock_semantics_source()->CreateSemanticNode(mock_semantic_provider()->koid(), std::move(node2));
  }
};

TEST_F(RecoverA11YFocusActionTest, FocusIsStillValid) {
  mock_a11y_focus_manager()->SetA11yFocus(mock_semantic_provider()->koid(), 0,
                                          [](bool result) { EXPECT_TRUE(result); });
  a11y::RecoverA11YFocusAction action(action_context(), mock_screen_reader_context());
  action.Run({});
  RunLoopUntilIdle();
  auto focus = mock_a11y_focus_manager()->GetA11yFocus();
  ASSERT_TRUE(focus);
  ASSERT_EQ(focus->view_ref_koid, mock_semantic_provider()->koid());
  ASSERT_EQ(focus->node_id, 0u);
  EXPECT_TRUE(mock_a11y_focus_manager()->IsRedrawHighlightsCalled());
}

TEST_F(RecoverA11YFocusActionTest, ViewChangeClearsPreviousNavigationContext) {
  // Set current navigation context to a different view.
  MockSemanticProvider semantic_provider_2(nullptr, nullptr);
  a11y::ScreenReaderContext::NavigationContext navigation_context;
  navigation_context.view_ref_koid = semantic_provider_2.koid();
  navigation_context.containers = {{.node_id = 2u}};
  mock_screen_reader_context()->set_current_navigation_context(navigation_context);

  mock_a11y_focus_manager()->SetA11yFocus(mock_semantic_provider()->koid(), 0,
                                          [](bool result) { EXPECT_TRUE(result); });
  a11y::RecoverA11YFocusAction action(action_context(), mock_screen_reader_context());
  action.Run({});
  RunLoopUntilIdle();
  auto focus = mock_a11y_focus_manager()->GetA11yFocus();
  ASSERT_TRUE(focus);
  ASSERT_EQ(focus->view_ref_koid, mock_semantic_provider()->koid());
  ASSERT_EQ(focus->node_id, 0u);
  EXPECT_TRUE(mock_a11y_focus_manager()->IsRedrawHighlightsCalled());
  const auto& previous_navigation_context =
      mock_screen_reader_context()->previous_navigation_context();
  EXPECT_FALSE(previous_navigation_context.view_ref_koid.has_value());
  EXPECT_TRUE(previous_navigation_context.containers.empty());
}

TEST_F(RecoverA11YFocusActionTest, InvalidFocusRecoversToFirstDescribableNode) {
  // Sets the focus to a node that does not exist, then run the action.
  mock_a11y_focus_manager()->SetA11yFocus(mock_semantic_provider()->koid(), 100,
                                          [](bool result) { EXPECT_TRUE(result); });
  // Set a fake navigation context to ensure that it's cleared when the screen
  // reader recovers to node 1, which does not belong to a container.
  a11y::ScreenReaderContext::NavigationContext navigation_context;
  navigation_context.containers = {{.node_id = 100u}};
  mock_screen_reader_context()->set_current_navigation_context(navigation_context);

  a11y::RecoverA11YFocusAction action(action_context(), mock_screen_reader_context());
  action.Run({});
  RunLoopUntilIdle();
  auto focus = mock_a11y_focus_manager()->GetA11yFocus();
  ASSERT_TRUE(focus);
  EXPECT_EQ(mock_semantic_provider()->koid(), focus->view_ref_koid);
  EXPECT_EQ(focus->node_id, 1u);
  EXPECT_EQ(mock_speaker()->speak_node_ids().size(), 1u);
  EXPECT_EQ(mock_speaker()->speak_node_ids()[0], 1u);
  EXPECT_TRUE(mock_screen_reader_context()->current_navigation_context().containers.empty());
}

TEST_F(RecoverA11YFocusActionTest, InvalidFocusTriesRestoringA11yFocusToInputFocusAndStops) {
  // Sets the focus to a koid that does not exist, then run the action.
  mock_a11y_focus_manager()->SetA11yFocus(NON_EXISTENT_KOID, 0,
                                          [](bool result) { EXPECT_TRUE(result); });
  // When RestoreA11yFocusToInputFocus is called, we'll get back to a valid
  // view, but still an invalid node. Then, we'll recover to node 1, which does not belong to a
  // container.
  mock_a11y_focus_manager()->set_restore_a11y_focus_to_input_focus_value(
      mock_semantic_provider()->koid(), INVALID_NODE_ID);

  a11y::RecoverA11YFocusAction action(action_context(), mock_screen_reader_context());
  action.Run({});
  RunLoopUntilIdle();

  EXPECT_TRUE(mock_a11y_focus_manager()->IsRestoreA11yFocusToInputFocusCalled());
  EXPECT_FALSE(mock_a11y_focus_manager()->IsClearA11yFocusCalled());
  auto focus = mock_a11y_focus_manager()->GetA11yFocus();
  ASSERT_TRUE(focus);
  EXPECT_EQ(mock_semantic_provider()->koid(), focus->view_ref_koid);
  EXPECT_EQ(focus->node_id, 1u);
}

TEST_F(RecoverA11YFocusActionTest, InvalidFocusTriesRestoringA11yFocusToInputFocusAndFails) {
  // Sets the focus to a koid that does not exist, then run the action.
  mock_a11y_focus_manager()->SetA11yFocus(NON_EXISTENT_KOID, 0,
                                          [](bool result) { EXPECT_TRUE(result); });
  // When RestoreA11yFocusToInputFocus is called, we'll still be on the bad focus.
  mock_a11y_focus_manager()->set_restore_a11y_focus_to_input_focus_value(NON_EXISTENT_KOID, 0);

  a11y::RecoverA11YFocusAction action(action_context(), mock_screen_reader_context());
  action.Run({});
  RunLoopUntilIdle();

  EXPECT_TRUE(mock_a11y_focus_manager()->IsRestoreA11yFocusToInputFocusCalled());
  EXPECT_TRUE(mock_a11y_focus_manager()->IsClearA11yFocusCalled());
  auto focus = mock_a11y_focus_manager()->GetA11yFocus();
  ASSERT_FALSE(focus);
}

}  // namespace
}  // namespace accessibility_test

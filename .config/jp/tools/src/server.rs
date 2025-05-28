use mcp_attr::{
    server::{mcp_server, McpServer, RequestContext},
    Result,
};

use crate::{github::State, ToolsServer};

#[mcp_server]
impl McpServer for ToolsServer {
    #[tool]
    /// Execute all unit and integration tests and build examples of the
    /// project.
    async fn cargo_test(
        &self,
        /// Package to run tests for.
        package: Option<String>,
        /// If specified, only run tests containing this string in their names.
        testname: Option<String>,
        ctx: &RequestContext,
    ) -> Result<String> {
        crate::cargo::test(package, testname, ctx).await
    }

    #[tool]
    /// Find one or more issues in the project's GitHub repository.
    async fn github_issues(
        &self,
        /// Issue number to get information about.
        ///
        /// If unspecified, a list of all issues will be returned, without the
        /// issue contents. You can re-run the tool with the correct issue
        /// number to get more details about an issue.
        number: Option<i32>,
    ) -> Result<String> {
        crate::github::issues(number.map(|v| v as u64)).await
    }

    #[tool]
    /// Find one or more pull requests in the project's GitHub repository.
    async fn github_pulls(
        &self,
        /// Pull request number to get information about.
        ///
        /// If unspecified, a list of all pull requests will be returned, without the
        /// pull request contents. You can re-run the tool with the correct pull request
        /// number to get more details about a pull request.
        number: Option<i32>,

        state: Option<State>,
    ) -> Result<String> {
        crate::github::pulls(number.map(|v| v as u64), state).await
    }
}

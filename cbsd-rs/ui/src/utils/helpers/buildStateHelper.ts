/*
 * Copyright © 2026 Clyso GmbH
 *
 *  Licensed under the GNU Affero General Public License, Version 3.0 (the "License");
 *  you may not use this file except in compliance with the License.
 *  You may obtain a copy of the License at
 *
 *  https://www.gnu.org/licenses/agpl-3.0.html
 *
 *  Unless required by applicable law or agreed to in writing, software
 *  distributed under the License is distributed on an "AS IS" BASIS,
 *  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 *  See the License for the specific language governing permissions and
 *  limitations under the License.
 */

import {
  ErrorBuildState,
  NeutralBuildState,
  SuccessBuildState,
  WarningBuildState,
  type BuildState,
} from '../types/cbs';

export abstract class BuildStateHelper {
  static toSentenceCase(buildState: BuildState): string {
    return (
      buildState.charAt(0).toUpperCase() + buildState.slice(1).toLowerCase()
    );
  }

  static isNeutralBuildState(buildState: BuildState): boolean {
    return Object.values(NeutralBuildState).includes(
      buildState as NeutralBuildState,
    );
  }

  static isSuccessBuildState(buildState: BuildState): boolean {
    return Object.values(SuccessBuildState).includes(
      buildState as SuccessBuildState,
    );
  }

  static isWarningBuildState(buildState: BuildState): boolean {
    return Object.values(WarningBuildState).includes(
      buildState as WarningBuildState,
    );
  }

  static isErrorBuildState(buildState: BuildState): boolean {
    return Object.values(ErrorBuildState).includes(
      buildState as ErrorBuildState,
    );
  }
}

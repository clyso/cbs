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

/*
 * Auth types
 */

export interface ApiTokenInfo {
  user: string;
  expires: string;
}

export interface ApiToken {
  token: string;
  info: ApiTokenInfo;
}

export interface User {
  email: string;
  name: string;
  token: ApiToken | null;
}

export type UserResponse = User;

/*
 * Build types
 */

export interface Build {
  task_id: string;
  desc: BuildDescription;
  user: string;
  submitted: string;
  state: BuildState;
  started: string;
  finished: string;
}

export enum NeutralBuildState {
  NEW = 'NEW',
  PENDING = 'PENDING',
  STARTED = 'STARTED',
  RETRY = 'RETRY',
}

export enum SuccessBuildState {
  SUCCESS = 'SUCCESS',
}

export enum WarningBuildState {
  REVOKED = 'REVOKED',
}

export enum ErrorBuildState {
  REJECTED = 'REJECTED',
  FAILURE = 'FAILURE',
}

export type BuildState =
  | NeutralBuildState
  | SuccessBuildState
  | WarningBuildState
  | ErrorBuildState;

export enum BuildVersionType {
  RELEASE = 'release',
  DEV = 'dev',
  TEST = 'test',
  CI = 'ci',
}

export enum BuildArch {
  ARM64 = 'arm64',
  X86_64 = 'x86_64',
}

export interface BuildDestImage {
  name: string;
  tag: string;
}

export interface BuildComponent {
  name: string;
  ref: string;
  repo: string | null;
}

export interface BuildTarget {
  distro: string;
  os_version: string;
  artifact_type: string;
  arch: BuildArch;
}

export interface SignedOffByUser {
  user: string;
  email: string;
}

export interface BuildDescription {
  version: string;
  channel: string;
  signed_off_by: SignedOffByUser;
  version_type: BuildVersionType;
  dst_image: BuildDestImage;
  components: BuildComponent[];
  build: BuildTarget;
}

export type BuildStatusResponse = [number, Build];

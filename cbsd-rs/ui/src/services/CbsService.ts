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

import { apiClient } from '@/http';
import { ApiHelper } from '@/utils/helpers/apiHelper';
import type { BuildStatusResponse, UserResponse } from '@/utils/types/cbs';

export abstract class CbsService {
  static login() {
    window.location.href = ApiHelper.getApiUrl(
      '/auth/login?next=/dashboard/home',
    );
  }

  static async getUser(): Promise<UserResponse> {
    const { data } = await apiClient.get(ApiHelper.getApiUrl('/auth/whoami'));

    return data;
  }

  static async getBuilds(all: boolean = false): Promise<BuildStatusResponse[]> {
    const params = { all };
    const { data } = await apiClient.get(
      ApiHelper.getApiUrl('/builds/status'),
      { params },
    );

    return data;
  }
}
